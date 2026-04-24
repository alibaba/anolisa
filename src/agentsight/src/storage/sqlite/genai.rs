//! GenAI semantic events SQLite storage
//!
//! Stores GenAI events (LLM calls, tool uses, etc.) to SQLite when SLS is not configured.
//! Implements the GenAIExporter trait for pluggable integration.
//!
//! # Size Limit
//!
//! The database size can be configured via `AGENTSIGHT_GENAI_DB_MAX_SIZE_MB` environment
//! variable (default: 200 MB). When approaching 90% of the limit, old records are pruned
//! automatically. The size check includes the main database file plus WAL and SHM files.

use std::path::PathBuf;
use std::sync::Mutex;
use rusqlite::{params, Connection};

use crate::genai::semantic::GenAISemanticEvent;
use crate::genai::exporter::GenAIExporter;
use super::connection::{create_connection, default_base_path};

// ─── Size limit configuration ──────────────────────────────────────────────────

/// Environment variable name for max database size in MB
const ENV_MAX_DB_SIZE_MB: &str = "AGENTSIGHT_GENAI_DB_MAX_SIZE_MB";
/// Default max database size: 200 MB
const DEFAULT_MAX_DB_SIZE_MB: u64 = 200;
/// Percentage of records to prune per attempt
const PRUNE_PERCENT: f64 = 0.05;
/// Maximum prune retry attempts to avoid infinite loop
const MAX_PRUNE_RETRIES: u32 = 3;

/// Get max database size from environment variable or use default
fn get_max_db_size() -> u64 {
    std::env::var(ENV_MAX_DB_SIZE_MB)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_DB_SIZE_MB)
        * 1024 * 1024
}

/// Get prune threshold (90% of max)
fn get_prune_threshold() -> u64 {
    (get_max_db_size() as f64 * 0.9) as u64
}

// ─── Query result types ────────────────────────────────────────────────────────

/// One data-point in a token time-series response
#[derive(Debug, serde::Serialize)]
pub struct TimeseriesBucket {
    pub bucket_start_ns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

/// One data-point in a per-model token time-series response
#[derive(Debug, serde::Serialize)]
pub struct ModelTimeseriesBucket {
    pub bucket_start_ns: i64,
    pub model: String,
    pub total_tokens: i64,
}

/// Per-agent token usage summary (all-time aggregation)
#[derive(Debug, serde::Serialize)]
pub struct AgentTokenSummary {
    pub agent_name: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub request_count: i64,
}

/// Summary of a single gen_ai.session_id within a time window
#[derive(Debug, serde::Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub conversation_count: i64,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub model: Option<String>,
    pub agent_name: Option<String>,
}

/// Session summary for the Token Savings page
#[derive(Debug, serde::Serialize)]
pub struct SavingsSessionSummary {
    pub session_id: String,
    pub agent_name: Option<String>,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

/// Summary of a single conversation (user query) within a session
#[derive(Debug, serde::Serialize)]
pub struct TraceSummary {
    pub trace_id: String,
    pub conversation_id: String,
    pub call_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub start_ns: i64,
    pub end_ns: Option<i64>,
    pub model: Option<String>,
    /// The first user_query string recorded in this conversation (best-effort)
    pub user_query: Option<String>,
}

/// One LLM call event within a trace
#[derive(Debug, serde::Serialize)]
pub struct TraceEventDetail {
    pub id: i64,
    pub call_id: Option<String>,
    pub start_timestamp_ns: i64,
    pub end_timestamp_ns: Option<i64>,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    /// JSON string of input messages array
    pub input_messages: Option<String>,
    /// JSON string of output messages array
    pub output_messages: Option<String>,
    /// JSON string of system instructions
    pub system_instructions: Option<String>,
    pub agent_name: Option<String>,
    pub process_name: Option<String>,
    pub pid: Option<i64>,
    /// The user query that triggered this LLM call
    pub user_query: Option<String>,
    /// Raw full event JSON stored at write time — used as fallback when
    /// output_messages is NULL (e.g. SSE streams that weren't fully parsed)
    pub event_json: Option<String>,
    /// Trace ID (LLM API response_id) — needed for session-level ATIF export
    /// to identify individual LLM calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Conversation ID (user query fingerprint) — groups multiple LLM calls
    /// triggered by the same user query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Cache read tokens — maps to ATIF cached_tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    /// Call lifecycle status: 'pending' | 'complete' | 'interrupted'
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Interruption type if abnormal: 'llm_error' | 'sse_truncated' | 'timeout' | etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interruption_type: Option<String>,
}

/// Lightweight info needed to write a PENDING record when a request is first seen
pub struct PendingCallInfo {
    /// Unique call ID (same one that will be used in the complete record)
    pub call_id: String,
    /// Trace ID (LLM API response_id, e.g. chatcmpl-xxx)
    pub trace_id: Option<String>,
    /// Conversation ID (user query fingerprint)
    pub conversation_id: Option<String>,
    /// Session ID
    pub session_id: Option<String>,
    /// Request start timestamp (nanoseconds)
    pub start_timestamp_ns: u64,
    /// Process ID
    pub pid: i32,
    /// Process name / comm
    pub process_name: String,
    /// Resolved agent name
    pub agent_name: Option<String>,
    /// HTTP method
    pub http_method: Option<String>,
    /// HTTP path
    pub http_path: Option<String>,
    /// Serialised input messages (JSON)
    pub input_messages: Option<String>,
    /// Serialised system instructions (JSON)
    pub system_instructions: Option<String>,
    /// User query extracted from request
    pub user_query: Option<String>,
    /// Whether this is an SSE streaming request
    pub is_sse: bool,
    /// Model name (extracted from request body)
    pub model: Option<String>,
    /// Provider name (extracted from request path)
    pub provider: Option<String>,
}

/// Data extracted from captured SSE events for enriching a pending record.
pub struct SseEnrichment {
    pub model: Option<String>,
    pub trace_id: Option<String>,
    pub provider: Option<String>,
    pub output_messages: Option<String>,
    pub sse_event_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

/// SQLite-backed GenAI event storage
pub struct GenAISqliteStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

impl GenAISqliteStore {
    /// Create a new GenAI SQLite store at the default path
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Self::default_path();
        Self::new_with_path(&path)
    }

    /// Create a new GenAI SQLite store at an arbitrary path
    pub fn new_with_path(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let conn = create_connection(path)?;
        let store = GenAISqliteStore {
            conn: Mutex::new(conn),
            db_path: path.to_path_buf(),
        };
        store.init_tables()?;
        
        // Log current database size on startup
        let current_size = store.get_total_db_size();
        let max_size = get_max_db_size();
        let threshold = get_prune_threshold();
        log::info!(
            "GenAISqliteStore initialized: db_size={}MB, threshold={}MB, max={}MB",
            current_size / 1024 / 1024,
            threshold / 1024 / 1024,
            max_size / 1024 / 1024
        );
        
        Ok(store)
    }

    /// Default database path
    pub fn default_path() -> PathBuf {
        default_base_path().join("genai_events.db")
    }

    /// Initialize database tables
    fn init_tables(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS genai_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                -- call lifecycle status: 'pending' | 'complete' | 'interrupted'
                -- 'pending'     : request captured, waiting for response
                -- 'complete'    : full request+response recorded
                -- 'interrupted' : response never arrived (crash / truncation)
                status TEXT NOT NULL DEFAULT 'complete',
                call_id TEXT,
                trace_id TEXT,
                conversation_id TEXT,
                session_id TEXT,
                instance TEXT,
                start_timestamp_ns INTEGER NOT NULL,
                end_timestamp_ns INTEGER,
                duration_ns INTEGER,
                pid INTEGER,
                process_name TEXT,
                agent_name TEXT,
                -- GenAI standard fields
                operation_name TEXT,
                provider TEXT,
                model TEXT,
                request_model TEXT,
                response_model TEXT,
                temperature REAL,
                max_tokens INTEGER,
                top_p REAL,
                frequency_penalty REAL,
                presence_penalty REAL,
                finish_reasons TEXT,
                server_address TEXT,
                -- Token usage
                input_tokens INTEGER,
                output_tokens INTEGER,
                total_tokens INTEGER,
                cache_creation_tokens INTEGER,
                cache_read_tokens INTEGER,
                -- Messages (JSON)
                system_instructions TEXT,
                input_messages TEXT,
                output_messages TEXT,
                -- AgentSight extensions
                user_query TEXT,
                http_method TEXT,
                http_path TEXT,
                status_code INTEGER,
                is_sse INTEGER,
                sse_event_count INTEGER,
                -- Interruption type detected for this call (nullable)
                interruption_type TEXT,
                -- Full event as JSON (fallback)
                event_json TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_genai_session_id ON genai_events(session_id);
            CREATE INDEX IF NOT EXISTS idx_genai_trace_id ON genai_events(trace_id);
            CREATE INDEX IF NOT EXISTS idx_genai_conversation_id ON genai_events(conversation_id);
            CREATE INDEX IF NOT EXISTS idx_genai_instance ON genai_events(instance);
            CREATE INDEX IF NOT EXISTS idx_genai_start_timestamp ON genai_events(start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_pid ON genai_events(pid);
            CREATE INDEX IF NOT EXISTS idx_genai_model ON genai_events(model);
            CREATE INDEX IF NOT EXISTS idx_genai_call_id ON genai_events(call_id);
            CREATE INDEX IF NOT EXISTS idx_genai_provider ON genai_events(provider);
            -- Composite indexes for common query patterns
            CREATE INDEX IF NOT EXISTS idx_genai_session_timestamp ON genai_events(session_id, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_trace_timestamp ON genai_events(trace_id, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_conversation_timestamp ON genai_events(conversation_id, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_pid_timestamp ON genai_events(pid, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_instance_timestamp ON genai_events(instance, start_timestamp_ns);",
            // NOTE: idx_genai_status and idx_genai_interruption_type are NOT created here
            // because they depend on columns added via migration. They are created in the
            // migration blocks below, which guarantees the columns exist first.
        )?;

        // ── Forward-compatible migrations ──────────────────────────────────────
        // Each block checks for a column's existence before ALTER TABLE, making
        // all migrations idempotent and safe to run on both old and new databases.
        // Columns are listed in the order they were added historically.

        // Query existing columns once to avoid repeated PRAGMA calls
        let existing_cols: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare(
                "SELECT name FROM pragma_table_info('genai_events')"
            )?;
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect()
        };

        // Helper macro: ALTER TABLE only if column absent, then always ensure index
        macro_rules! ensure_col {
            // Column with no index
            ($col:literal, $def:literal) => {
                if !existing_cols.contains($col) {
                    conn.execute_batch(
                        &format!("ALTER TABLE genai_events ADD COLUMN {} {};", $col, $def)
                    )?;
                    log::info!("Migrated genai_events: added '{}' column", $col);
                }
            };
            // Column + index
            ($col:literal, $def:literal, $idx:literal) => {
                if !existing_cols.contains($col) {
                    conn.execute_batch(
                        &format!("ALTER TABLE genai_events ADD COLUMN {} {};", $col, $def)
                    )?;
                    log::info!("Migrated genai_events: added '{}' column", $col);
                }
                // Always run CREATE INDEX IF NOT EXISTS — safe even if index already exists
                conn.execute_batch(
                    &format!("CREATE INDEX IF NOT EXISTS {} ON genai_events({});", $idx, $col)
                )?;
            };
        }

        // v2: Anthropic prompt-cache token counters
        ensure_col!("cache_creation_tokens", "INTEGER");
        ensure_col!("cache_read_tokens",     "INTEGER");

        // v3: two-phase write lifecycle status
        ensure_col!("status", "TEXT NOT NULL DEFAULT 'complete'",
                    "idx_genai_status");

        // v4: per-call interruption type
        ensure_col!("interruption_type", "TEXT",
                    "idx_genai_interruption_type");


        // Migration: add conversation_id column for existing databases
        let _ = conn.execute("ALTER TABLE genai_events ADD COLUMN conversation_id TEXT", []);

        Ok(())
    }

    // ─── Pending-call lifecycle methods ────────────────────────────────────────

    /// Insert a PENDING record as soon as a request is captured.
    ///
    /// The record is later promoted to 'complete' via [`complete_pending`] once
    /// the full response arrives, or marked 'interrupted' by the stale-scan thread
    /// if the agent crashes before the response is received.
    pub fn insert_pending(&self, info: &PendingCallInfo) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let instance = crate::genai::sls::SlsUploader::get_instance_id();
        conn.execute(
            "INSERT INTO genai_events (
                event_type, status, call_id, trace_id, conversation_id, session_id, instance,
                start_timestamp_ns, pid, process_name, agent_name,
                http_method, http_path, is_sse,
                input_messages, system_instructions, user_query,
                model, provider,
                event_json
            ) VALUES (
                'llm_call', 'pending', ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9,
                ?10, ?11, ?12,
                ?13, ?14, ?15,
                ?16, ?17,
                '{}'
            )",
            params![
                info.call_id,
                info.trace_id,
                info.conversation_id,
                info.session_id,
                instance,
                info.start_timestamp_ns as i64,
                info.pid,
                info.process_name,
                info.agent_name,
                info.http_method,
                info.http_path,
                if info.is_sse { 1i64 } else { 0 },
                info.input_messages,
                info.system_instructions,
                info.user_query,
                info.model,
                info.provider,
            ],
        )?;
        Ok(())
    }

    /// Promote an existing PENDING record to 'complete' by updating all response fields.
    ///
    /// If no matching PENDING row exists (e.g. because the DB was restarted), the
    /// call falls back to a plain INSERT so data is never silently dropped.
    pub fn complete_pending(
        &self,
        event: &GenAISemanticEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let GenAISemanticEvent::LLMCall(call) = event {
            {
                let conn = self.conn.lock().unwrap();
                let event_json = serde_json::to_string(event)?;

                let (input_tokens, output_tokens, total_tokens) = call.token_usage.as_ref()
                    .map(|u| (u.input_tokens as i64, u.output_tokens as i64, u.total_tokens as i64))
                    .unwrap_or((0, 0, 0));
                let cache_creation = call.token_usage.as_ref()
                    .and_then(|u| u.cache_creation_input_tokens.map(|v| v as i64));
                let cache_read = call.token_usage.as_ref()
                    .and_then(|u| u.cache_read_input_tokens.map(|v| v as i64));

                let system_instructions: Option<String> = {
                    let sys: Vec<_> = call.request.messages.iter()
                        .filter(|m| m.role == "system").collect();
                    if sys.is_empty() { None } else { serde_json::to_string(&sys).ok() }
                };
                let input_messages: Option<String> = {
                    let non_sys: Vec<_> = call.request.messages.iter()
                        .filter(|m| m.role != "system").collect();
                    let latest = if let Some(idx) = non_sys.iter().rposition(|m| m.role == "user") {
                        &non_sys[idx..]
                    } else { &non_sys[..] };
                    if latest.is_empty() { None } else { serde_json::to_string(&latest).ok() }
                };
                let output_messages: Option<String> = if call.response.messages.is_empty() {
                    None
                } else {
                    serde_json::to_string(&call.response.messages).ok()
                };
                let finish_reasons: Option<String> = {
                    let reasons: Vec<_> = call.response.messages.iter()
                        .filter_map(|m| m.finish_reason.as_deref()).collect();
                    if reasons.is_empty() { None } else { serde_json::to_string(&reasons).ok() }
                };

                let updated = conn.execute(
                    "UPDATE genai_events SET
                        status = 'complete',
                        trace_id            = ?1,
                        conversation_id     = ?2,
                        end_timestamp_ns    = ?3,
                        duration_ns         = ?4,
                        provider            = ?5,
                        model               = ?6,
                        request_model       = ?7,
                        response_model      = ?8,
                        temperature         = ?9,
                        max_tokens          = ?10,
                        top_p               = ?11,
                        frequency_penalty   = ?12,
                        presence_penalty    = ?13,
                        finish_reasons      = ?14,
                        server_address      = ?15,
                        input_tokens        = ?16,
                        output_tokens       = ?17,
                        total_tokens        = ?18,
                        cache_creation_tokens = ?19,
                        cache_read_tokens   = ?20,
                        system_instructions = ?21,
                        input_messages      = ?22,
                        output_messages     = ?23,
                        status_code         = ?24,
                        sse_event_count     = ?25,
                        event_json          = ?26
                    WHERE call_id = ?27 AND status = 'pending'",
                    params![
                        call.metadata.get("response_id"),
                        call.metadata.get("conversation_id"),
                        call.end_timestamp_ns as i64,
                        call.duration_ns as i64,
                        call.provider,
                        call.model,
                        call.model,
                        call.model,
                        call.request.temperature,
                        call.request.max_tokens.map(|v| v as i64),
                        call.request.top_p,
                        call.request.frequency_penalty,
                        call.request.presence_penalty,
                        finish_reasons,
                        call.metadata.get("server.address"),
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        cache_creation,
                        cache_read,
                        system_instructions,
                        input_messages,
                        output_messages,
                        call.metadata.get("status_code").and_then(|s| s.parse::<i64>().ok()),
                        call.metadata.get("sse_event_count").and_then(|s| s.parse::<i64>().ok()),
                        event_json,
                        call.call_id,
                    ],
                )?;

                if updated > 0 {
                    log::debug!("[GenAI] Promoted pending→complete for call_id={}", call.call_id);
                    return Ok(());
                }
                // No pending row found — fall through to plain insert below
                log::debug!("[GenAI] No pending row for call_id={}, inserting directly", call.call_id);
            }
            // Fallback: store_event handles the full INSERT path
            self.store_event(event)
        } else {
            // Non-LLMCall events have no pending lifecycle, store directly
            self.store_event(event)
        }
    }

    /// Mark stale PENDING records as 'interrupted'.
    ///
    /// Called by the background scanner.  Any `llm_call` row that has been in
    /// 'pending' state for longer than `timeout_secs` is assumed to have been
    /// lost (agent crash / network cut) and is updated to 'interrupted'.
    ///
    /// Returns the number of rows updated.
    pub fn mark_interrupted_stale(
        &self,
        timeout_secs: u64,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let cutoff_ns = {
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            now_ns - (timeout_secs as i64 * 1_000_000_000)
        };
        let updated = conn.execute(
            "UPDATE genai_events
             SET status = 'interrupted'
             WHERE event_type = 'llm_call'
               AND status = 'pending'
               AND start_timestamp_ns < ?1",
            params![cutoff_ns],
        )?;
        if updated > 0 {
            log::info!("[GenAI] Marked {} stale pending call(s) as interrupted", updated);
        }
        Ok(updated)
    }

    /// Set the interruption_type for a specific call_id.
    ///
    /// Called by the online InterruptionDetector after detecting an anomaly.
    pub fn update_interruption_type(
        &self,
        call_id: &str,
        itype: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE genai_events SET interruption_type = ?1 WHERE call_id = ?2",
            params![itype, call_id],
        )?;
        Ok(())
    }

    /// List all pending calls for a specific PID.
    ///
    /// Returns (call_id, session_id, trace_id) tuples for all PENDING records
    /// matching the given PID. Used by HealthChecker to link agent_crash events
    /// to their associated LLM calls.
    pub fn list_pending_for_pid(
        &self,
        pid: i32,
    ) -> Result<Vec<(String, Option<String>, Option<String>)>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT call_id, session_id, trace_id
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND status = 'pending'
               AND pid = ?1",
        )?;
        let rows = stmt.query_map(params![pid], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }

    /// Mark all pending calls for a PID as interrupted.
    ///
    /// Called by HealthChecker when it detects an agent process has gone offline.
    /// Sets status='interrupted' and interruption_type to the provided value.
    pub fn mark_pending_interrupted_for_pid(
        &self,
        pid: i32,
        itype: &str,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE genai_events
             SET status = 'interrupted', interruption_type = ?1
             WHERE event_type = 'llm_call'
               AND status = 'pending'
               AND pid = ?2",
            params![itype, pid],
        )?;
        if updated > 0 {
            log::info!("Marked {} pending call(s) as interrupted for pid={}", updated, pid);
        }
        Ok(updated)
    }

    /// Find sessions/traces whose agentic loop was cut short by a process crash.
    ///
    /// Looks for the most recent `complete` LLM call for each (session, trace) pair
    /// associated with the given PID where the finish reason is `tool_calls` —
    /// meaning the model issued another tool call and was still mid-loop when the
    /// process died.  These sessions never produced a final answer and should be
    /// surfaced as `agent_crash` interruptions even though every individual call
    /// completed successfully.
    ///
    /// Returns `(call_id, session_id, trace_id)` tuples, deduplicated by
    /// (session_id, trace_id) so one interruption event is emitted per trace.
    pub fn list_incomplete_agentic_sessions_for_pid(
        &self,
        pid: i32,
        since_ns: i64,
    ) -> Result<Vec<(String, Option<String>, Option<String>)>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        // For each (session_id, trace_id) group belonging to this pid, find the
        // call with the highest start_timestamp_ns.  If that call's finish_reason
        // contains 'tool_calls' the agentic loop was never concluded.
        let mut stmt = conn.prepare(
            "SELECT call_id, session_id, trace_id
             FROM genai_events g1
             WHERE g1.event_type = 'llm_call'
               AND g1.status    = 'complete'
               AND g1.pid       = ?1
               AND g1.start_timestamp_ns >= ?2
               AND g1.finish_reasons LIKE '%tool_calls%'
               AND g1.start_timestamp_ns = (
                   SELECT MAX(g2.start_timestamp_ns)
                   FROM genai_events g2
                   WHERE g2.event_type = 'llm_call'
                     AND g2.status     = 'complete'
                     AND g2.pid        = ?1
                     AND g2.start_timestamp_ns >= ?2
                     AND COALESCE(g2.session_id, '') = COALESCE(g1.session_id, '')
                     AND COALESCE(g2.trace_id,   '') = COALESCE(g1.trace_id,   '')
               )",
        )?;
        let rows = stmt.query_map(params![pid, since_ns], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }

    /// Look up the real session_id from completed records for the same PID.
    /// Used in drain path to reconcile SHA256-hash fallback session_id with the
    /// real agent UUID from ResponseSessionMapper.
    pub fn lookup_session_for_pid(
        &self,
        pid: i32,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT session_id FROM genai_events
             WHERE pid = ?1 AND status = 'complete' AND session_id IS NOT NULL
             ORDER BY start_timestamp_ns DESC LIMIT 1",
            params![pid],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(sid) => Ok(Some(sid)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Box::new(e)),
        }
    }

    /// Update the session_id of a pending record after reconciliation.
    pub fn update_session_id(
        &self,
        call_id: &str,
        session_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE genai_events SET session_id = ?2 WHERE call_id = ?1",
            params![call_id, session_id],
        )?;
        Ok(())
    }

    /// Enrich a pending record with data extracted from captured SSE events.
    /// Updates model, trace_id, provider, output_messages, sse_event_count, and token counts.
    pub fn enrich_pending_from_sse(
        &self,
        call_id: &str,
        enrichment: &SseEnrichment,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE genai_events SET
                model            = COALESCE(?2, model),
                trace_id         = COALESCE(?3, trace_id),
                provider         = COALESCE(?4, provider),
                output_messages  = COALESCE(?5, output_messages),
                sse_event_count  = COALESCE(?6, sse_event_count),
                input_tokens     = COALESCE(?7, input_tokens),
                output_tokens    = COALESCE(?8, output_tokens)
             WHERE call_id = ?1",
            params![
                call_id,
                enrichment.model,
                enrichment.trace_id,
                enrichment.provider,
                enrichment.output_messages,
                enrichment.sse_event_count,
                enrichment.input_tokens,
                enrichment.output_tokens,
            ],
        )?;
        Ok(())
    }

    // ─── Query methods ───────────────────────────────────────────────────────

    /// List all sessions within a nanosecond timestamp range.
    pub fn list_sessions(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<SessionSummary>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id,
                    COUNT(DISTINCT conversation_id) AS conversation_count,
                    MIN(start_timestamp_ns)  AS first_seen_ns,
                    MAX(start_timestamp_ns)  AS last_seen_ns,
                    COALESCE(SUM(input_tokens), 0)  AS total_input,
                    COALESCE(SUM(output_tokens), 0) AS total_output,
                    MAX(model)               AS model,
                    MAX(agent_name)          AS agent_name
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND session_id IS NOT NULL
               AND start_timestamp_ns BETWEEN ?1 AND ?2
             GROUP BY session_id
             ORDER BY last_seen_ns DESC",
        )?;
        let rows = stmt.query_map(params![start_ns, end_ns], |row| {
            Ok(SessionSummary {
                session_id: row.get(0)?,
                conversation_count: row.get(1)?,
                first_seen_ns: row.get(2)?,
                last_seen_ns: row.get(3)?,
                total_input_tokens: row.get(4)?,
                total_output_tokens: row.get(5)?,
                model: row.get(6)?,
                agent_name: row.get(7)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// List sessions for the Token Savings page.
    ///
    /// Independent from `list_sessions()` to avoid affecting existing functionality.
    /// Supports optional agent_name filtering directly in SQL.
    pub fn list_sessions_for_savings(
        &self,
        start_ns: i64,
        end_ns: i64,
        agent_name: Option<&str>,
    ) -> Result<Vec<SavingsSessionSummary>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();

        let sql = if agent_name.is_some() {
            "SELECT session_id,
                    MAX(agent_name)                  AS agent_name,
                    COALESCE(SUM(input_tokens), 0)   AS total_input,
                    COALESCE(SUM(output_tokens), 0)  AS total_output
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND session_id IS NOT NULL
               AND start_timestamp_ns BETWEEN ?1 AND ?2
               AND agent_name = ?3
             GROUP BY session_id
             ORDER BY MAX(start_timestamp_ns) DESC"
        } else {
            "SELECT session_id,
                    MAX(agent_name)                  AS agent_name,
                    COALESCE(SUM(input_tokens), 0)   AS total_input,
                    COALESCE(SUM(output_tokens), 0)  AS total_output
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND session_id IS NOT NULL
               AND start_timestamp_ns BETWEEN ?1 AND ?2
             GROUP BY session_id
             ORDER BY MAX(start_timestamp_ns) DESC"
        };

        let mut stmt = conn.prepare(sql)?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<SavingsSessionSummary> {
            Ok(SavingsSessionSummary {
                session_id: row.get(0)?,
                agent_name: row.get(1)?,
                total_input_tokens: row.get(2)?,
                total_output_tokens: row.get(3)?,
            })
        };

        let rows = if let Some(name) = agent_name {
            stmt.query_map(params![start_ns, end_ns, name], map_row)?
        } else {
            stmt.query_map(params![start_ns, end_ns], map_row)?
        };

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// List all conversations under a given session, with aggregated token stats.
    pub fn list_traces_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<TraceSummary>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT conversation_id,
                    COUNT(*)                        AS call_count,
                    COALESCE(SUM(input_tokens), 0)  AS total_input,
                    COALESCE(SUM(output_tokens), 0) AS total_output,
                    MIN(start_timestamp_ns)         AS start_ns,
                    MAX(end_timestamp_ns)           AS end_ns,
                    MAX(model)                      AS model,
                    MIN(user_query)                 AS user_query
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND session_id = ?1
               AND conversation_id IS NOT NULL
             GROUP BY conversation_id
             ORDER BY start_ns ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            let cid: String = row.get(0)?;
            Ok(TraceSummary {
                trace_id: cid.clone(),
                conversation_id: cid,
                call_count: row.get(1)?,
                total_input_tokens: row.get(2)?,
                total_output_tokens: row.get(3)?,
                start_ns: row.get(4)?,
                end_ns: row.get(5)?,
                model: row.get(6)?,
                user_query: row.get(7)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// List all distinct agent_name values observed in the given time window.
    pub fn list_agent_names(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT agent_name
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND agent_name IS NOT NULL
               AND start_timestamp_ns BETWEEN ?1 AND ?2
             ORDER BY agent_name ASC",
        )?;
        let rows = stmt.query_map(params![start_ns, end_ns], |row| {
            row.get::<_, String>(0)
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// One bucket in a token time-series query.
    pub fn get_token_timeseries(
        &self,
        start_ns: i64,
        end_ns: i64,
        agent_name: Option<&str>,
        bucket_count: u32,
    ) -> Result<Vec<TimeseriesBucket>, Box<dyn std::error::Error>> {
        let bucket_count = bucket_count.max(1);
        let range_ns = (end_ns - start_ns).max(1);
        let bucket_ns = range_ns / bucket_count as i64;

        let conn = self.conn.lock().unwrap();

        // Build query with optional agent_name filter
        let sql = if agent_name.is_some() {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(SUM(input_tokens), 0)            AS input_tokens,
                COALESCE(SUM(output_tokens), 0)           AS output_tokens,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
               AND agent_name = ?4
             GROUP BY bucket_idx
             ORDER BY bucket_idx ASC"
        } else {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(SUM(input_tokens), 0)            AS input_tokens,
                COALESCE(SUM(output_tokens), 0)           AS output_tokens,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
             GROUP BY bucket_idx
             ORDER BY bucket_idx ASC"
        };

        let rows: Vec<TimeseriesBucket> = if let Some(name) = agent_name {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns, name], |row| {
                Ok(TimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    total_tokens: row.get(4)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns], |row| {
                Ok(TimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    total_tokens: row.get(4)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?
        };

        Ok(rows)
    }

    /// Model-level token breakdown time-series.
    pub fn get_model_timeseries(
        &self,
        start_ns: i64,
        end_ns: i64,
        agent_name: Option<&str>,
        bucket_count: u32,
    ) -> Result<Vec<ModelTimeseriesBucket>, Box<dyn std::error::Error>> {
        let bucket_count = bucket_count.max(1);
        let range_ns = (end_ns - start_ns).max(1);
        let bucket_ns = range_ns / bucket_count as i64;

        let conn = self.conn.lock().unwrap();

        let sql = if agent_name.is_some() {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(model, 'unknown')                 AS model,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
               AND agent_name = ?4
             GROUP BY bucket_idx, model
             ORDER BY bucket_idx ASC"
        } else {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(model, 'unknown')                 AS model,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
             GROUP BY bucket_idx, model
             ORDER BY bucket_idx ASC"
        };

        let rows: Vec<ModelTimeseriesBucket> = if let Some(name) = agent_name {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns, name], |row| {
                Ok(ModelTimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    model: row.get(2)?,
                    total_tokens: row.get(3)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns], |row| {
                Ok(ModelTimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    model: row.get(2)?,
                    total_tokens: row.get(3)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?
        };

        Ok(rows)
    }

    /// Return per-agent token usage aggregated over all recorded history.
    ///
    /// Groups by `COALESCE(agent_name, process_name, 'unknown')` so that every
    /// LLM call is attributed to some label even when agent_name is NULL.
    pub fn get_agent_token_summary(
        &self,
    ) -> Result<Vec<AgentTokenSummary>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(agent_name, process_name, 'unknown') AS agent,
                    COALESCE(SUM(input_tokens),  0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(total_tokens),  0) AS total_tokens,
                    COUNT(*)                        AS request_count
             FROM genai_events
             WHERE event_type = 'llm_call'
             GROUP BY agent
             ORDER BY total_tokens DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AgentTokenSummary {
                agent_name:    row.get(0)?,
                input_tokens:  row.get(1)?,
                output_tokens: row.get(2)?,
                total_tokens:  row.get(3)?,
                request_count: row.get(4)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Fetch all LLM call events for a given trace ID (response_id).
    pub fn get_trace_events(
        &self,
        trace_id: &str,
    ) -> Result<Vec<TraceEventDetail>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE trace_id = ?1
               AND event_type = 'llm_call'
             ORDER BY start_timestamp_ns ASC",
        )?;
        let rows = stmt.query_map(params![trace_id], |row| {
            Ok(TraceEventDetail {
                id: row.get(0)?,
                call_id: row.get(1)?,
                start_timestamp_ns: row.get(2)?,
                end_timestamp_ns: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                input_messages: row.get(8)?,
                output_messages: row.get(9)?,
                system_instructions: row.get(10)?,
                agent_name: row.get(11)?,
                process_name: row.get(12)?,
                pid: row.get(13)?,
                user_query: row.get(14)?,
                event_json: row.get(15)?,
                trace_id: row.get(16)?,
                cache_read_tokens: row.get(17)?,
                conversation_id: row.get(18)?,
                status: row.get(19)?,
                interruption_type: row.get(20)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Fetch all LLM call events for a given conversation ID (user query fingerprint).
    pub fn get_events_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<TraceEventDetail>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE conversation_id = ?1
               AND event_type = 'llm_call'
             ORDER BY start_timestamp_ns ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok(TraceEventDetail {
                id: row.get(0)?,
                call_id: row.get(1)?,
                start_timestamp_ns: row.get(2)?,
                end_timestamp_ns: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                input_messages: row.get(8)?,
                output_messages: row.get(9)?,
                system_instructions: row.get(10)?,
                agent_name: row.get(11)?,
                process_name: row.get(12)?,
                pid: row.get(13)?,
                user_query: row.get(14)?,
                event_json: row.get(15)?,
                trace_id: row.get(16)?,
                cache_read_tokens: row.get(17)?,
                conversation_id: row.get(18)?,
                status: row.get(19)?,
                interruption_type: row.get(20)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Fetch all LLM call events for a given session ID (across all traces).
    pub fn get_events_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<TraceEventDetail>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE session_id = ?1
               AND event_type = 'llm_call'
             ORDER BY start_timestamp_ns ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(TraceEventDetail {
                id: row.get(0)?,
                call_id: row.get(1)?,
                start_timestamp_ns: row.get(2)?,
                end_timestamp_ns: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                input_messages: row.get(8)?,
                output_messages: row.get(9)?,
                system_instructions: row.get(10)?,
                agent_name: row.get(11)?,
                process_name: row.get(12)?,
                pid: row.get(13)?,
                user_query: row.get(14)?,
                event_json: row.get(15)?,
                trace_id: row.get(16)?,
                cache_read_tokens: row.get(17)?,
                conversation_id: row.get(18)?,
                status: row.get(19)?,
                interruption_type: row.get(20)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Store a single GenAI event with size limit enforcement
    fn store_event(&self, event: &GenAISemanticEvent) -> Result<(), Box<dyn std::error::Error>> {
        // Check size before write and prune if needed
        self.check_and_prune_if_needed()?;

        // Attempt insert with retry on SQLITE_FULL
        let mut retries = 0;
        loop {
            match self.try_insert_event(event) {
                Ok(()) => {
                    // Success: execute checkpoint to flush WAL to main DB
                    self.checkpoint()?;
                    return Ok(());
                }
                Err(e) => {
                    // Check if it's SQLITE_FULL (extended code 13)
                    if let Some(rusqlite::Error::SqliteFailure(err, _)) = 
                        e.downcast_ref::<rusqlite::Error>() {
                        if err.extended_code == 13 && retries < MAX_PRUNE_RETRIES {
                            retries += 1;
                            log::warn!(
                                "Database full (SQLITE_FULL), pruning old records (attempt {}/{})",
                                retries, MAX_PRUNE_RETRIES
                            );
                            self.prune_old_records()?;
                            self.checkpoint()?;
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Try to insert an event without size check
    fn try_insert_event(&self, event: &GenAISemanticEvent) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let event_json = serde_json::to_string(event)?;

        match event {
            GenAISemanticEvent::LLMCall(call) => {
                let (input_tokens, output_tokens, total_tokens) = call.token_usage.as_ref()
                    .map(|u| (u.input_tokens as i64, u.output_tokens as i64, u.total_tokens as i64))
                    .unwrap_or((0, 0, 0));
                let cache_creation = call.token_usage.as_ref()
                    .and_then(|u| u.cache_creation_input_tokens.map(|v| v as i64));
                let cache_read = call.token_usage.as_ref()
                    .and_then(|u| u.cache_read_input_tokens.map(|v| v as i64));

                // Extract system instructions
                let system_instructions: Option<String> = {
                    let sys_msgs: Vec<_> = call.request.messages.iter()
                        .filter(|m| m.role == "system")
                        .collect();
                    if sys_msgs.is_empty() { None }
                    else { serde_json::to_string(&sys_msgs).ok() }
                };

                // Extract input messages (incremental: latest round only)
                let input_messages: Option<String> = {
                    let non_system: Vec<_> = call.request.messages.iter()
                        .filter(|m| m.role != "system")
                        .collect();
                    let latest = if let Some(idx) = non_system.iter().rposition(|m| m.role == "user") {
                        &non_system[idx..]
                    } else {
                        &non_system[..]
                    };
                    if latest.is_empty() { None }
                    else { serde_json::to_string(&latest).ok() }
                };

                // Extract output messages
                let output_messages: Option<String> = if call.response.messages.is_empty() {
                    None
                } else {
                    serde_json::to_string(&call.response.messages).ok()
                };

                // Extract finish reasons
                let finish_reasons: Option<String> = if call.response.messages.is_empty() {
                    None
                } else {
                    let reasons: Vec<_> = call.response.messages.iter()
                        .filter_map(|m| m.finish_reason.as_deref())
                        .collect();
                    if reasons.is_empty() { None }
                    else { serde_json::to_string(&reasons).ok() }
                };

                // Get instance ID (same logic as SLS uploader)
                let instance = crate::genai::sls::SlsUploader::get_instance_id();

                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, call_id, trace_id, conversation_id, session_id, instance,
                        start_timestamp_ns, end_timestamp_ns, duration_ns, pid, process_name, agent_name,
                        operation_name, provider, model, request_model, response_model,
                        temperature, max_tokens, top_p, frequency_penalty, presence_penalty,
                        finish_reasons, server_address,
                        input_tokens, output_tokens, total_tokens,
                        cache_creation_tokens, cache_read_tokens,
                        system_instructions, input_messages, output_messages,
                        user_query, http_method, http_path, status_code,
                        is_sse, sse_event_count, event_json
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                        ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32,
                        ?33, ?34, ?35, ?36, ?37, ?38, ?39
                    )",
                    params![
                        "llm_call",
                        call.call_id,
                        call.metadata.get("response_id"),
                        call.metadata.get("conversation_id"),
                        call.metadata.get("session_id"),
                        instance,
                        call.start_timestamp_ns as i64,
                        call.end_timestamp_ns as i64,
                        call.duration_ns as i64,
                        call.pid,
                        call.process_name,
                        call.agent_name,
                        call.metadata.get("operation_name"),
                        call.provider,
                        call.model,
                        call.model, // request_model
                        call.model, // response_model (same for now)
                        call.request.temperature,
                        call.request.max_tokens.map(|v| v as i64),
                        call.request.top_p,
                        call.request.frequency_penalty,
                        call.request.presence_penalty,
                        finish_reasons,
                        call.metadata.get("server.address"),
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        cache_creation,
                        cache_read,
                        system_instructions,
                        input_messages,
                        output_messages,
                        call.metadata.get("user_query"),
                        call.metadata.get("method"),
                        call.metadata.get("path"),
                        call.metadata.get("status_code").and_then(|s| s.parse::<i64>().ok()),
                        call.metadata.get("is_sse").map(|s| if s == "true" { 1i64 } else { 0 }),
                        call.metadata.get("sse_event_count").and_then(|s| s.parse::<i64>().ok()),
                        event_json,
                    ],
                )?;
            }
            GenAISemanticEvent::ToolUse(tool) => {
                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, call_id, timestamp_ns, pid,
                        event_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        "tool_use",
                        tool.parent_llm_call_id,
                        tool.timestamp_ns as i64,
                        tool.pid,
                        event_json,
                    ],
                )?;
            }
            GenAISemanticEvent::AgentInteraction(interaction) => {
                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, timestamp_ns, pid,
                        event_json
                    ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        "agent_interaction",
                        interaction.timestamp_ns as i64,
                        interaction.pid,
                        event_json,
                    ],
                )?;
            }
            GenAISemanticEvent::StreamChunk(chunk) => {
                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, call_id, timestamp_ns, pid,
                        event_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        "stream_chunk",
                        chunk.parent_llm_call_id,
                        chunk.timestamp_ns as i64,
                        chunk.pid,
                        event_json,
                    ],
                )?;
            }
        }
        Ok(())
    }

    // ─── Size limit methods ───────────────────────────────────────────────────

    /// Get total database size (main db + wal + shm)
    fn get_total_db_size(&self) -> u64 {
        let mut total = 0u64;
        
        // Main database file
        if let Ok(meta) = std::fs::metadata(&self.db_path) {
            total += meta.len();
        }
        
        // WAL file
        let wal_path = format!("{}-wal", self.db_path.display());
        if let Ok(meta) = std::fs::metadata(&wal_path) {
            total += meta.len();
        }
        
        // SHM file
        let shm_path = format!("{}-shm", self.db_path.display());
        if let Ok(meta) = std::fs::metadata(&shm_path) {
            total += meta.len();
        }
        
        total
    }

    /// Check database size and prune if approaching limit
    /// 
    /// Keeps pruning until size drops below threshold to avoid repeated triggers.
    fn check_and_prune_if_needed(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut current_size = self.get_total_db_size();
        let threshold = get_prune_threshold();
        
        if current_size < threshold {
            return Ok(());
        }
        
        log::info!(
            "Database size {}MB exceeding threshold {}MB, pruning old records",
            current_size / 1024 / 1024,
            threshold / 1024 / 1024
        );
        
        // Keep pruning until below threshold (max 5 iterations to prevent infinite loop)
        let mut iterations = 0;
        while current_size >= threshold && iterations < 5 {
            iterations += 1;
            self.prune_old_records()?;
            self.checkpoint()?;
            current_size = self.get_total_db_size();
            
            if current_size >= threshold {
                log::info!(
                    "Database still {}MB (threshold {}MB), continue pruning (iteration {})",
                    current_size / 1024 / 1024,
                    threshold / 1024 / 1024,
                    iterations
                );
            }
        }
        
        log::info!(
            "Pruning complete, database size now {}MB",
            current_size / 1024 / 1024
        );
        
        Ok(())
    }

    /// Prune old records to free up space
    /// 
    /// Deletes a percentage of oldest records based on id.
    fn prune_old_records(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        
        // Get total count
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM genai_events",
            [],
            |row| row.get(0)
        )?;
        
        if count == 0 {
            return Ok(());
        }
        
        // Calculate how many to delete (5% of total)
        let delete_count = ((count as f64) * PRUNE_PERCENT).max(1.0) as i64;
        
        log::info!(
            "Pruning {} of {} records ({:.1}%)",
            delete_count, count, PRUNE_PERCENT * 100.0
        );
        
        // Delete oldest records by id
        let deleted = conn.execute(
            "DELETE FROM genai_events WHERE id IN (
                SELECT id FROM genai_events ORDER BY id ASC LIMIT ?1
            )",
            params![delete_count]
        )?;
        
        log::info!("Deleted {} records", deleted);
        
        Ok(())
    }

    /// Execute WAL checkpoint and VACUUM to reclaim disk space
    /// 
    /// 1. VACUUM: rebuild database to compact data
    /// 2. Checkpoint: flush and truncate WAL file
    /// 
    /// Note: VACUUM in WAL mode creates a new db file, so we need to
    /// re-enable WAL and checkpoint after VACUUM.
    fn checkpoint(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        
        // VACUUM rebuilds the database (works better before checkpoint in WAL mode)
        conn.execute_batch("VACUUM;")?;
        
        // Re-enable WAL mode (VACUUM may reset it)
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        
        // Checkpoint with TRUNCATE to shrink WAL file
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        
        Ok(())
    }
}

impl GenAIExporter for GenAISqliteStore {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn export(&self, events: &[GenAISemanticEvent]) {
        for event in events {
            if let Err(e) = self.store_event(event) {
                log::warn!("Failed to store GenAI event to SQLite: {}", e);
            }
        }
    }
}
