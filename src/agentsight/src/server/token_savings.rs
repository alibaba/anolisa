//! Token Savings API handlers
//!
//! Provides endpoints that cross-reference genai_events.db with the external
//! ~/.tokenless/stats.db to compute token savings metrics.

use actix_web::{HttpResponse, Responder, get, web};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::storage::sqlite::GenAISqliteStore;
use crate::storage::sqlite::tokenless::{self, TokenlessStatsStore};

// ─── Query parameters ────────────────────────────────────────────────────────

/// Query parameters for /api/token-savings
#[derive(Debug, Deserialize)]
pub struct TokenSavingsQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub agent_name: Option<String>,
}

// ─── Response DTOs ───────────────────────────────────────────────────────────

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

// ─── Mapping helpers ─────────────────────────────────────────────────────────

/// Map stats.db `operation` field to frontend category.
///
/// Classification rationale:
/// - `compress-response` / `compress-toon`: both compress MCP server responses
///   (toon uses a structured encoding variant), hence `mcp_response`.
/// - `rewrite-command` / `compress-schema`: both reduce tool-definition /
///   invocation payloads sent to the LLM, hence `tool_output`.
fn map_operation_to_category(operation: &str) -> &str {
    match operation {
        // MCP response compression strategies
        "compress-response" | "compress-toon" => "mcp_response",
        // Tool definition / invocation compression strategies
        "rewrite-command" | "compress-schema" => "tool_output",
        _ => "tool_output",
    }
}

/// Map operation to a human-readable title.
fn map_operation_to_title(operation: &str) -> &str {
    match operation {
        "compress-response" => "MCP响应压缩",
        "rewrite-command" => "工具输出优化",
        "compress-schema" => "Schema 压缩",
        "compress-toon" => "TOON 编码",
        _ => "其他优化",
    }
}

/// Map operation to a human-readable strategy label.
///
/// Note: unknown operations all map to "其他优化", and the aggregation logic
/// uses this label as the grouping key to avoid duplicate pie chart slices.
fn map_operation_to_strategy_label(operation: &str) -> &str {
    match operation {
        "compress-schema" => "Schema 压缩",
        "compress-response" => "响应压缩",
        "rewrite-command" => "命令重写",
        "compress-toon" => "TOON 编码",
        _ => "其他优化",
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

// ─── GET /api/token-savings ──────────────────────────────────────────────────

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
    // FIX(#2): aggregate by strategy *label* (not raw operation) so that
    // unknown operations merge into a single "其他优化" slice in the pie chart.
    let mut grand_strategy_map: std::collections::HashMap<String, (String, i64, i64)> =
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

                // FIX(#2): aggregate by strategy key so unknown ops merge into one slice.
                // Use operation name as key for known ops (frontend STRATEGY_CONFIG matches on this),
                // and "other" for unknown ops so they collapse into a single slice.
                let strategy_key = match row.operation.as_str() {
                    "compress-response" | "compress-toon" | "rewrite-command"
                    | "compress-schema" => row.operation.clone(),
                    _ => "other".to_string(),
                };
                let entry = grand_strategy_map.entry(strategy_key).or_insert((
                    strategy_label.clone(),
                    0,
                    0,
                ));
                entry.1 += saved;
                entry.2 += compounded;

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
                    before_summary: format!("原始内容 {} tokens", row.before_tokens),
                    after_summary: format!("优化后 {} tokens", row.after_tokens),
                    before_text: row.before_text.clone(),
                    after_text: row.after_text.clone(),
                    diff_lines,
                });
            }
        }

        // FIX(#1): use compounded/total_tokens for both list and detail pages
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

    // FIX(#2): strategy = operation key (for frontend color lookup),
    //           label = Chinese display name
    let strategy_breakdown: Vec<StrategyBreakdown> = grand_strategy_map
        .into_iter()
        .map(
            |(strategy_key, (label, saved, compounded_saved))| StrategyBreakdown {
                strategy: strategy_key,
                label,
                saved,
                compounded_saved,
            },
        )
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

// ─── GET /api/token-savings/session/{session_id} ─────────────────────────────

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

    // FIX(#3): query single session by id instead of full-table scan
    let store = match GenAISqliteStore::new_with_path(db_path) {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };

    let session = match store.get_session_for_savings(&session_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
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
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
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
                before_summary: format!("原始内容 {} tokens", row.before_tokens),
                after_summary: format!("优化后 {} tokens", row.after_tokens),
                before_text: row.before_text.clone(),
                after_text: row.after_text.clone(),
                diff_lines: Vec::new(),
            });
        }
    }

    // FIX(#1): use compounded/total_tokens — consistent with get_token_savings
    let savings_rate = if total_tokens > 0 {
        total_compounded_saved as f64 / total_tokens as f64 * 100.0
    } else {
        0.0
    };

    HttpResponse::Ok().json(SessionSavingsDetail {
        session_id,
        stats_available,
        total_actual_tokens: total_tokens,
        total_compounded_saved,
        total_original_tokens: total_tokens + total_compounded_saved,
        savings_rate,
        items,
    })
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
            security_observability: crate::server::SecurityObservabilityConfig::default(),
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
        assert_eq!(map_operation_to_title("compress-response"), "MCP响应压缩");
        assert_eq!(map_operation_to_title("rewrite-command"), "工具输出优化");
        assert_eq!(map_operation_to_title("compress-schema"), "Schema 压缩");
        assert_eq!(map_operation_to_title("compress-toon"), "TOON 编码");
        assert_eq!(map_operation_to_title("other"), "其他优化");
    }

    #[test]
    fn test_map_operation_to_strategy_label() {
        assert_eq!(
            map_operation_to_strategy_label("compress-schema"),
            "Schema 压缩"
        );
        assert_eq!(
            map_operation_to_strategy_label("compress-response"),
            "响应压缩"
        );
        assert_eq!(
            map_operation_to_strategy_label("rewrite-command"),
            "命令重写"
        );
        assert_eq!(
            map_operation_to_strategy_label("compress-toon"),
            "TOON 编码"
        );
        assert_eq!(map_operation_to_strategy_label("unknown"), "其他优化");
    }

    // ─── Integration tests for handlers ───────────────────────────────────

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_token_savings_no_stats_db() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let orig_home = std::env::var("HOME").ok();
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

        // Restore HOME to avoid polluting other tests
        match orig_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_token_savings_with_stats() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let orig_home = std::env::var("HOME").ok();
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

        // Restore HOME
        match orig_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_session_savings_not_found() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let orig_home = std::env::var("HOME").ok();
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

        // Restore HOME
        match orig_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[allow(clippy::await_holding_lock)]
    #[actix_web::test]
    async fn test_get_session_savings_with_data() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let orig_home = std::env::var("HOME").ok();
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

        // Restore HOME
        match orig_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
