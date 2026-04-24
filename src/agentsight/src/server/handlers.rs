//! API request handlers

use actix_web::{delete, get, post, web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::health::AgentHealthStatus;
use crate::storage::sqlite::{GenAISqliteStore};
use crate::storage::sqlite::genai::{TimeseriesBucket, ModelTimeseriesBucket};
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
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64); // 24 h

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.list_sessions(start_ns, end_ns) {
            Ok(sessions) => HttpResponse::Ok().json(sessions),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
    }
}

/// GET /api/sessions/{session_id}/traces
///
/// Returns all conversations belonging to a session with token stats.
#[get("/api/sessions/{session_id}/traces")]
pub async fn list_traces_by_session(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let db_path = &data.storage_path;
    let session_id = path.into_inner();

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.list_traces_by_session(&session_id) {
            Ok(traces) => HttpResponse::Ok().json(traces),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.list_agent_names(start_ns, end_ns) {
            Ok(names) => HttpResponse::Ok().json(names),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64);
    let buckets = query.buckets.unwrap_or(30);
    let agent_name = query.agent_name.as_deref();

    match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => {
            let token_series = match store.get_token_timeseries(start_ns, end_ns, agent_name, buckets) {
                Ok(v) => v,
                Err(e) => return HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": e.to_string()})),
            };
            let model_series = match store.get_model_timeseries(start_ns, end_ns, agent_name, buckets) {
                Ok(v) => v,
                Err(e) => return HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": e.to_string()})),
            };
            HttpResponse::Ok().json(TimeseriesResponse { token_series, model_series })
        }
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
                    .body(format!("# ERROR querying metrics: {}\n", e));
            }
        },
        Err(e) => {
            return HttpResponse::InternalServerError()
                .content_type("text/plain; version=0.0.4")
                .body(format!("# ERROR opening database: {}\n", e));
        }
    };

    let mut out = String::with_capacity(512 + summaries.len() * 128);

    // agentsight_token_input_total
    out.push_str("# HELP agentsight_token_input_total Total input tokens consumed by agent (all-time)\n");
    out.push_str("# TYPE agentsight_token_input_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_token_input_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name), s.input_tokens
        ));
    }
    out.push('\n');

    // agentsight_token_output_total
    out.push_str("# HELP agentsight_token_output_total Total output tokens consumed by agent (all-time)\n");
    out.push_str("# TYPE agentsight_token_output_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_token_output_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name), s.output_tokens
        ));
    }
    out.push('\n');

    // agentsight_token_total_total
    out.push_str("# HELP agentsight_token_total_total Total tokens (input+output) consumed by agent (all-time)\n");
    out.push_str("# TYPE agentsight_token_total_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_token_total_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name), s.total_tokens
        ));
    }
    out.push('\n');

    // agentsight_llm_requests_total
    out.push_str("# HELP agentsight_llm_requests_total Total LLM requests made by agent (all-time)\n");
    out.push_str("# TYPE agentsight_llm_requests_total counter\n");
    for s in &summaries {
        out.push_str(&format!(
            "agentsight_llm_requests_total{{agent=\"{}\"}} {}\n",
            escape_label(&s.agent_name), s.request_count
        ));
    }
    out.push('\n');

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
#[get("/api/agent-health")]
pub async fn get_agent_health(data: web::Data<AppState>) -> impl Responder {
    let store = data.health_store.read().unwrap();
    HttpResponse::Ok().json(AgentHealthResponse {
        agents: store.all_agents(),
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
        store.all_agents()
            .into_iter()
            .find(|a| a.pid == pid)
            .and_then(|a| a.restart_cmd)
    };

    let cmd = match restart_cmd {
        Some(c) if !c.is_empty() => c,
        _ => return HttpResponse::BadRequest()
            .json(serde_json::json!({"error": "no restart command available for this pid"})),
    };

    // Step 1: kill -9
    use std::process::Command;
    let kill_result = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();

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
            log::info!(
                "Restarted agent pid={} -> new pid={}, cmd={:?}",
                pid, new_pid, cmd
            );
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
                .json(serde_json::json!({"error": e.to_string()}))
        }
    };

    let events = match store.get_trace_events(&trace_id) {
        Ok(e) => e,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}))
        }
    };

    if events.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "trace not found"}));
    }

    match crate::atif::convert_trace_to_atif(&trace_id, events) {
        Ok(doc) => HttpResponse::Ok().json(doc),
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
                .json(serde_json::json!({"error": e.to_string()}))
        }
    };

    let events = match store.get_events_by_session(&session_id) {
        Ok(e) => e,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}))
        }
    };

    if events.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "session not found"}));
    }

    match crate::atif::convert_session_to_atif(&session_id, events) {
        Ok(doc) => HttpResponse::Ok().json(doc),
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
                .json(serde_json::json!({"error": e.to_string()}))
        }
    };

    let events = match store.get_events_by_conversation(&conversation_id) {
        Ok(e) => e,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}))
        }
    };

    if events.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "conversation not found"}));
    }

    match crate::atif::convert_trace_to_atif(&conversation_id, events) {
        Ok(doc) => HttpResponse::Ok().json(doc),
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
    }
}

// ─── Interruption endpoints ────────────────────────────────────────────────────

/// Query parameters for /api/interruptions
#[derive(Debug, Deserialize)]
pub struct InterruptionQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub agent_name: Option<String>,
    /// Filter by type: llm_error | sse_truncated | timeout | agent_crash | token_limit | context_overflow | tool_incomplete
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

    let end_ns   = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64); // 24 h
    let limit    = query.limit.unwrap_or(200);

    match istore.list(
        start_ns, end_ns,
        query.agent_name.as_deref(),
        query.interruption_type.as_deref(),
        query.severity.as_deref(),
        query.resolved,
        limit,
    ) {
        Ok(rows) => HttpResponse::Ok().json(rows),
        Err(e)   => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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

    let end_ns   = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

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
                    "high"     => high     += s.count as u64,
                    "medium"   => medium   += s.count as u64,
                    _          => low      += s.count as u64,
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
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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

    let end_ns   = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match istore.stats(start_ns, end_ns) {
        Ok(stats) => HttpResponse::Ok().json(stats),
        Err(e)    => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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

    let end_ns   = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match istore.count_unresolved_by_session_detailed(start_ns, end_ns) {
        Ok(rows) => {
            // Group by session_id
            let mut map: std::collections::HashMap<String, (i64, std::collections::HashMap<String, i64>, Vec<serde_json::Value>)> = std::collections::HashMap::new();
            for (sid, severity, itype, cnt) in rows {
                let entry = map.entry(sid).or_insert_with(|| (0, std::collections::HashMap::new(), Vec::new()));
                entry.0 += cnt;
                *entry.1.entry(severity.clone()).or_insert(0) += cnt;
                entry.2.push(serde_json::json!({
                    "interruption_type": itype,
                    "severity": severity,
                    "count": cnt,
                }));
            }
            let json: Vec<_> = map.into_iter().map(|(sid, (total, by_sev, types))| {
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
            }).collect();
            HttpResponse::Ok().json(json)
        }
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
    }
}

/// GET /api/interruptions/trace-counts?start_ns=<i64>&end_ns=<i64>
///
/// Returns unresolved interruption breakdown per trace_id, grouped by severity and type.
/// Response: [ { trace_id, total, by_severity: { critical, high, medium, low },
///              types: [ { interruption_type, severity, count }, ... ] }, ... ]
#[get("/api/interruptions/trace-counts")]
pub async fn interruption_trace_counts(
    data: web::Data<AppState>,
    query: web::Query<InterruptionQuery>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let end_ns   = query.end_ns.unwrap_or_else(|| now_ns() as i64);
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64);

    match istore.count_unresolved_by_trace_detailed(start_ns, end_ns) {
        Ok(rows) => {
            let mut map: std::collections::HashMap<String, (i64, std::collections::HashMap<String, i64>, Vec<serde_json::Value>)> = std::collections::HashMap::new();
            for (tid, severity, itype, cnt) in rows {
                let entry = map.entry(tid).or_insert_with(|| (0, std::collections::HashMap::new(), Vec::new()));
                entry.0 += cnt;
                *entry.1.entry(severity.clone()).or_insert(0) += cnt;
                entry.2.push(serde_json::json!({
                    "interruption_type": itype,
                    "severity": severity,
                    "count": cnt,
                }));
            }
            let json: Vec<_> = map.into_iter().map(|(tid, (total, by_sev, types))| {
                serde_json::json!({
                    "trace_id": tid,
                    "total": total,
                    "by_severity": {
                        "critical": by_sev.get("critical").copied().unwrap_or(0),
                        "high": by_sev.get("high").copied().unwrap_or(0),
                        "medium": by_sev.get("medium").copied().unwrap_or(0),
                        "low": by_sev.get("low").copied().unwrap_or(0),
                    },
                    "types": types,
                })
            }).collect();
            HttpResponse::Ok().json(json)
        }
        Err(e) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
        Err(e)   => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
    }
}

/// GET /api/traces/{trace_id}/interruptions
///
/// Returns all interruption events for a specific trace.
#[get("/api/traces/{trace_id}/interruptions")]
pub async fn list_trace_interruptions(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let Some(ref istore) = data.interruption_store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "Interruption store not initialized"}));
    };

    let trace_id = path.into_inner();
    match istore.list_by_trace(&trace_id) {
        Ok(rows) => HttpResponse::Ok().json(rows),
        Err(e)   => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
        Ok(true)  => HttpResponse::Ok().json(serde_json::json!({"status": "resolved"})),
        Ok(false) => HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Interruption not found"})),
        Err(e)    => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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
        Ok(None)      => HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Interruption not found"})),
        Err(e)        => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
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

/// Overall savings summary
#[derive(Debug, Serialize)]
pub struct SavingsSummary {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_saved_tokens: i64,
    pub savings_rate: f64,
    pub total_tool_saved: i64,
    pub total_mcp_saved: i64,
}

/// A single optimization item within a session
#[derive(Debug, Serialize)]
pub struct OptimizationItemDto {
    pub id: String,
    pub category: String,
    pub title: String,
    pub before_tokens: i64,
    pub after_tokens: i64,
    pub saved_tokens: i64,
    pub before_summary: String,
    pub after_summary: String,
    pub diff_lines: Vec<DiffLineDto>,
}

/// A single diff line
#[derive(Debug, Serialize)]
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
    pub savings_rate: f64,
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

/// Parse a unified-diff-style text into DiffLine entries.
fn parse_diff_text(text: &str) -> Vec<DiffLineDto> {
    text.lines()
        .map(|line| {
            if let Some(content) = line.strip_prefix('+') {
                DiffLineDto { line_type: "add".into(), content: content.to_string() }
            } else if let Some(content) = line.strip_prefix('-') {
                DiffLineDto { line_type: "remove".into(), content: content.to_string() }
            } else {
                DiffLineDto { line_type: "context".into(), content: line.to_string() }
            }
        })
        .collect()
}

/// Map stats.db operation field to frontend category.
fn map_operation_to_category(operation: &str) -> &str {
    match operation {
        "compress-response" => "mcp_response",
        "rewrite-command" => "tool_output",
        _ => "tool_output",
    }
}

/// Map operation to a human-readable title.
fn map_operation_to_title(operation: &str) -> &str {
    match operation {
        "compress-response" => "MCP\u{54cd}\u{5e94}\u{538b}\u{7f29}",
        "rewrite-command" => "\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}",
        _ => "\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}",
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
    let start_ns = query.start_ns.unwrap_or_else(|| end_ns - 86_400_000_000_000i64);
    let agent_name = query.agent_name.as_deref();

    // Step 1: Query sessions from genai_events.db
    let sessions = match GenAISqliteStore::new_with_path(db_path) {
        Ok(store) => match store.list_sessions_for_savings(start_ns, end_ns, agent_name) {
            Ok(s) => s,
            Err(e) => return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Err(e) => return HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": e.to_string()})),
    };

    // Step 2: Open stats.db (read-only, graceful if absent)
    let stats_path = tokenless::default_stats_path();
    let stats_store = TokenlessStatsStore::open_if_exists(&stats_path);
    let stats_available = stats_store.is_some();

    // Step 3: Batch-query optimization records by session_id
    let stats_by_session = if let Some(ref store) = stats_store {
        let session_ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        let rows = store.get_stats_by_session_ids(&session_ids);
        TokenlessStatsStore::group_by_session(rows)
    } else {
        std::collections::HashMap::new()
    };

    // Step 4: Build response
    let mut resp_sessions = Vec::with_capacity(sessions.len());
    let mut grand_input: i64 = 0;
    let mut grand_output: i64 = 0;
    let mut grand_saved: i64 = 0;
    let mut grand_tool_saved: i64 = 0;
    let mut grand_mcp_saved: i64 = 0;

    for session in &sessions {
        let total_tokens = session.total_input_tokens + session.total_output_tokens;
        let mut session_saved: i64 = 0;
        let mut session_tool_saved: i64 = 0;
        let mut session_mcp_saved: i64 = 0;
        let mut items = Vec::new();

        if let Some(stat_rows) = stats_by_session.get(&session.session_id) {
            for row in stat_rows {
                let saved = row.before_tokens - row.after_tokens;
                let category = map_operation_to_category(&row.operation);
                let title = map_operation_to_title(&row.operation);

                if category == "mcp_response" {
                    session_mcp_saved += saved;
                } else {
                    session_tool_saved += saved;
                }
                session_saved += saved;

                let diff_lines = row.diff_text.as_deref()
                    .map(parse_diff_text)
                    .unwrap_or_default();

                items.push(OptimizationItemDto {
                    id: row.tool_call_id.clone(),
                    category: category.to_string(),
                    title: title.to_string(),
                    before_tokens: row.before_tokens,
                    after_tokens: row.after_tokens,
                    saved_tokens: saved,
                    before_summary: format!("\u{539f}\u{59cb}\u{5185}\u{5bb9} {} tokens", row.before_tokens),
                    after_summary: format!("\u{4f18}\u{5316}\u{540e} {} tokens", row.after_tokens),
                    diff_lines,
                });
            }
        }

        let savings_rate = if total_tokens > 0 {
            session_saved as f64 / total_tokens as f64 * 100.0
        } else {
            0.0
        };

        grand_input += session.total_input_tokens;
        grand_output += session.total_output_tokens;
        grand_saved += session_saved;
        grand_tool_saved += session_tool_saved;
        grand_mcp_saved += session_mcp_saved;

        resp_sessions.push(SessionSavingsDto {
            session_id: session.session_id.clone(),
            agent_name: session.agent_name.clone().unwrap_or_default(),
            total_input_tokens: session.total_input_tokens,
            total_output_tokens: session.total_output_tokens,
            total_tokens,
            saved_tokens: session_saved,
            savings_rate,
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

    HttpResponse::Ok().json(TokenSavingsResponse {
        stats_available,
        summary: SavingsSummary {
            total_input_tokens: grand_input,
            total_output_tokens: grand_output,
            total_tokens: grand_total,
            total_saved_tokens: grand_saved,
            savings_rate: grand_rate,
            total_tool_saved: grand_tool_saved,
            total_mcp_saved: grand_mcp_saved,
        },
        sessions: resp_sessions,
    })
}
