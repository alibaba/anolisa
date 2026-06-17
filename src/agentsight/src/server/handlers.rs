//! API request handlers

use actix_web::{HttpResponse, Responder, get, post, web};
use serde::{Deserialize, Serialize};

use super::AppState;
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
pub async fn get_agent_health(data: web::Data<AppState>) -> impl Responder {
    let store = data.health_store.read().unwrap();
    let agents = store
        .all_agents()
        .into_iter()
        .filter(|a| a.agent_name != "Cosh")
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
    pub compounded_saved: i64,
    pub compounding_turns: i64,
    pub compression_ratio: f64,
    pub explanation: String,
    pub before_summary: String,
    pub after_summary: String,
    pub before_text: Option<String>,
    pub after_text: Option<String>,
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
    pub compounded_saved: i64,
    pub savings_rate: f64,
    pub compounded_savings_rate: f64,
    pub request_count: i64,
    pub tool_saved: i64,
    pub mcp_saved: i64,
    pub optimization_items: Vec<OptimizationItemDto>,
}

/// An actionable optimization tip
#[derive(Debug, Serialize)]
pub struct OptimizationTip {
    pub level: String,
    pub title: String,
    pub description: String,
}

/// Full response for /api/token-savings
#[derive(Debug, Serialize)]
pub struct TokenSavingsResponse {
    pub stats_available: bool,
    pub summary: SavingsSummary,
    pub sessions: Vec<SessionSavingsDto>,
    pub optimization_tips: Vec<OptimizationTip>,
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

                let compression_ratio = if row.before_tokens > 0 {
                    (1.0 - row.after_tokens as f64 / row.before_tokens as f64) * 100.0
                } else {
                    0.0
                };

                let explanation = if category == "mcp_response" {
                    format!(
                        "MCP\u{54cd}\u{5e94}\u{538b}\u{7f29}: \u{539f}\u{59cb} {} tokens \u{2192} {} tokens\u{ff0c}\u{538b}\u{7f29}\u{7387} {:.1}%\u{3002}\u{540e}\u{7eed} {} \u{8f6e}LLM\u{8c03}\u{7528}\u{5747}\u{53d7}\u{76ca}\u{ff0c}\u{590d}\u{5408}\u{8282}\u{7701} {} tokens\u{3002}",
                        row.before_tokens, row.after_tokens, compression_ratio, compounding_turns, compounded
                    )
                } else {
                    format!(
                        "\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}: \u{539f}\u{59cb} {} tokens \u{2192} {} tokens\u{ff0c}\u{538b}\u{7f29}\u{7387} {:.1}%\u{3002}\u{540e}\u{7eed} {} \u{8f6e}LLM\u{8c03}\u{7528}\u{5747}\u{53d7}\u{76ca}\u{ff0c}\u{590d}\u{5408}\u{8282}\u{7701} {} tokens\u{3002}",
                        row.before_tokens, row.after_tokens, compression_ratio, compounding_turns, compounded
                    )
                };

                items.push(OptimizationItemDto {
                    id: row.tool_use_id.clone(),
                    category: category.to_string(),
                    title: title.to_string(),
                    before_tokens: row.before_tokens,
                    after_tokens: row.after_tokens,
                    saved_tokens: saved,
                    compounded_saved: compounded,
                    compounding_turns,
                    compression_ratio,
                    explanation,
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

    // ── Generate optimization tips ──────────────────────────────────────────
    let mut optimization_tips: Vec<OptimizationTip> = Vec::new();

    if !stats_available {
        optimization_tips.push(OptimizationTip {
            level: "warning".to_string(),
            title: "\u{672a}\u{68c0}\u{6d4b}\u{5230} Tokenless \u{7ec4}\u{4ef6}".to_string(),
            description: "\u{672a}\u{53d1}\u{73b0} stats.db\u{ff0c}\u{8bf7}\u{786e}\u{8ba4} tokenless \u{7ec4}\u{4ef6}\u{5df2}\u{5b89}\u{88c5}\u{5e76}\u{542f}\u{7528}\u{3002}\u{542f}\u{7528}\u{540e}\u{53ef}\u{81ea}\u{52a8}\u{538b}\u{7f29}\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{548c} MCP \u{54cd}\u{5e94}\u{ff0c}\u{663e}\u{8457}\u{964d}\u{4f4e} Token \u{6d88}\u{8017}\u{3002}".to_string(),
        });
    } else if grand_compounded_rate < 5.0 && grand_total > 0 {
        optimization_tips.push(OptimizationTip {
            level: "warning".to_string(),
            title: "\u{8282}\u{7701}\u{7387}\u{8f83}\u{4f4e}".to_string(),
            description: "\u{5f53}\u{524d}\u{590d}\u{5408}\u{8282}\u{7701}\u{7387}\u{4e0d}\u{8db3} 5%\u{ff0c}\u{5efa}\u{8bae}\u{68c0}\u{67e5} tokenless \u{914d}\u{7f6e}\u{662f}\u{5426}\u{5df2}\u{5bf9}\u{6240}\u{6709} Agent \u{751f}\u{6548}\u{ff0c}\u{786e}\u{4fdd}\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{548c} MCP \u{54cd}\u{5e94}\u{538b}\u{7f29}\u{5747}\u{5df2}\u{5f00}\u{542f}\u{3002}".to_string(),
        });
    }

    if grand_compounded_tool_saved > 0 && grand_compounded_mcp_saved == 0 && grand_total > 0 {
        optimization_tips.push(OptimizationTip {
            level: "info".to_string(),
            title: "\u{5efa}\u{8bae}\u{5f00}\u{542f} MCP \u{54cd}\u{5e94}\u{538b}\u{7f29}".to_string(),
            description: "\u{5f53}\u{524d}\u{4ec5}\u{6709}\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}\u{ff0c}\u{672a}\u{68c0}\u{6d4b}\u{5230} MCP \u{54cd}\u{5e94}\u{538b}\u{7f29}\u{3002}\u{5f00}\u{542f}\u{540e}\u{53ef}\u{8fdb}\u{4e00}\u{6b65}\u{964d}\u{4f4e} Token \u{6d88}\u{8017}\u{3002}".to_string(),
        });
    }

    if grand_compounded_mcp_saved > 0 && grand_compounded_tool_saved == 0 && grand_total > 0 {
        optimization_tips.push(OptimizationTip {
            level: "info".to_string(),
            title: "\u{5efa}\u{8bae}\u{5f00}\u{542f}\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}".to_string(),
            description: "\u{5f53}\u{524d}\u{4ec5}\u{6709} MCP \u{54cd}\u{5e94}\u{538b}\u{7f29}\u{ff0c}\u{672a}\u{68c0}\u{6d4b}\u{5230}\u{5de5}\u{5177}\u{8f93}\u{51fa}\u{4f18}\u{5316}\u{3002}\u{5f00}\u{542f}\u{540e}\u{53ef}\u{8fdb}\u{4e00}\u{6b65}\u{964d}\u{4f4e} Token \u{6d88}\u{8017}\u{3002}".to_string(),
        });
    }

    // Tip for sessions with zero savings
    let zero_savings_sessions = resp_sessions.iter().filter(|s| s.compounded_saved == 0 && s.total_tokens > 1000).count();
    if zero_savings_sessions > 0 {
        optimization_tips.push(OptimizationTip {
            level: "info".to_string(),
            title: format!("\u{53d1}\u{73b0} {} \u{4e2a}\u{672a}\u{4f18}\u{5316}\u{4f1a}\u{8bdd}", zero_savings_sessions),
            description: "\u{90e8}\u{5206}\u{4f1a}\u{8bdd}\u{6d88}\u{8017}\u{8f83}\u{9ad8}\u{4f46}\u{65e0}\u{4f18}\u{5316}\u{8bb0}\u{5f55}\u{ff0c}\u{53ef}\u{80fd}\u{662f}\u{5bf9}\u{5e94} Agent \u{672a}\u{542f}\u{7528} tokenless \u{6216}\u{5de5}\u{5177}\u{8c03}\u{7528}\u{8f83}\u{5c11}\u{3002}\u{5efa}\u{8bae}\u{68c0}\u{67e5}\u{8fd9}\u{4e9b}\u{4f1a}\u{8bdd}\u{7684} Agent \u{914d}\u{7f6e}\u{3002}".to_string(),
        });
    }

    if grand_compounded_rate >= 30.0 {
        optimization_tips.push(OptimizationTip {
            level: "success".to_string(),
            title: "\u{8282}\u{7701}\u{6548}\u{679c}\u{4f18}\u{79c0}".to_string(),
            description: format!("\u{5f53}\u{524d}\u{590d}\u{5408}\u{8282}\u{7701}\u{7387} {:.1}%\u{ff0c}\u{8868}\u{73b0}\u{4f18}\u{79c0}\u{ff01}\u{7ee7}\u{7eed}\u{4fdd}\u{6301}\u{5f53}\u{524d}\u{914d}\u{7f6e}\u{3002}", grand_compounded_rate),
        });
    } else if grand_compounded_rate >= 15.0 {
        optimization_tips.push(OptimizationTip {
            level: "success".to_string(),
            title: "\u{8282}\u{7701}\u{6548}\u{679c}\u{826f}\u{597d}".to_string(),
            description: format!("\u{5f53}\u{524d}\u{590d}\u{5408}\u{8282}\u{7701}\u{7387} {:.1}%\u{ff0c}\u{5df2}\u{8fbe}\u{5230}\u{826f}\u{597d}\u{6c34}\u{5e73}\u{3002}\u{53ef}\u{5c1d}\u{8bd5}\u{8c03}\u{6574}\u{538b}\u{7f29}\u{7b56}\u{7565}\u{4ee5}\u{8fdb}\u{4e00}\u{6b65}\u{63d0}\u{5347}\u{3002}", grand_compounded_rate),
        });
    }

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
        },
        sessions: resp_sessions,
        optimization_tips,
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
