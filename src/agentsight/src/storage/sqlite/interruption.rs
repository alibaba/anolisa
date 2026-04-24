//! SQLite storage for interruption_events table.

use rusqlite::{params, Connection};
use std::sync::Mutex;

use crate::interruption::{InterruptionEvent, InterruptionType, Severity};
use super::connection::create_connection;

// ─── API response types ────────────────────────────────────────────────────────

/// Summary returned by GET /api/interruptions
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterruptionRecord {
    pub id: i64,
    pub interruption_id: String,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub call_id: Option<String>,
    pub pid: Option<i64>,
    pub agent_name: Option<String>,
    pub interruption_type: String,
    pub severity: String,
    pub occurred_at_ns: i64,
    pub detail: Option<String>,
    pub resolved: bool,
}

/// Per-type count for stats endpoint
#[derive(Debug, serde::Serialize)]
pub struct InterruptionTypeStat {
    pub interruption_type: String,
    pub severity: String,
    pub count: i64,
}

// ─── Store ────────────────────────────────────────────────────────────────────

pub struct InterruptionStore {
    conn: Mutex<Connection>,
}

impl InterruptionStore {
    pub fn new_with_path(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let conn = create_connection(path)?;
        let store = InterruptionStore { conn: Mutex::new(conn) };
        store.init_tables()?;
        Ok(store)
    }

    fn init_tables(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS interruption_events (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                interruption_id     TEXT NOT NULL UNIQUE,
                session_id          TEXT,
                trace_id            TEXT,
                call_id             TEXT,
                pid                 INTEGER,
                agent_name          TEXT,
                interruption_type   TEXT NOT NULL,
                severity            TEXT NOT NULL,
                occurred_at_ns      INTEGER NOT NULL,
                detail              TEXT,
                resolved            INTEGER NOT NULL DEFAULT 0,
                created_at          DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_interruption_session  ON interruption_events(session_id);
            CREATE INDEX IF NOT EXISTS idx_interruption_occurred ON interruption_events(occurred_at_ns);
            CREATE INDEX IF NOT EXISTS idx_interruption_type     ON interruption_events(interruption_type);
            CREATE INDEX IF NOT EXISTS idx_interruption_agent    ON interruption_events(agent_name);
            CREATE INDEX IF NOT EXISTS idx_interruption_resolved ON interruption_events(resolved);",
        )?;
        Ok(())
    }

    // ─── Write ──────────────────────────────────────────────────────────────

    /// Insert a single interruption event (ignores duplicates by interruption_id).
    pub fn insert(&self, event: &InterruptionEvent) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO interruption_events (
                interruption_id, session_id, trace_id, call_id, pid, agent_name,
                interruption_type, severity, occurred_at_ns, detail, resolved
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                event.interruption_id,
                event.session_id,
                event.trace_id,
                event.call_id,
                event.pid,
                event.agent_name,
                event.interruption_type.as_str(),
                event.severity.as_str(),
                event.occurred_at_ns,
                event.detail,
                event.resolved as i32,
            ],
        )?;
        Ok(())
    }

    /// Insert multiple events, ignoring duplicates.
    pub fn insert_batch(&self, events: &[InterruptionEvent]) -> Result<(), Box<dyn std::error::Error>> {
        for e in events {
            self.insert(e)?;
        }
        Ok(())
    }

    /// Deduplication check for OOM events: return true if an agent_crash row
    /// with oom=true already exists for the given (pid, occurred_at_ns).
    pub fn oom_event_exists(&self, pid: i32, occurred_at_ns: i64) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM interruption_events
             WHERE interruption_type='agent_crash' AND pid=?1 AND occurred_at_ns=?2
               AND detail LIKE '%\"oom\":true%'",
            params![pid, occurred_at_ns],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0) > 0
    }

    /// Return the maximum occurred_at_ns of OOM-sourced agent_crash events.
    /// Returns 0 if no such events exist.
    pub fn latest_oom_event_ns(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(occurred_at_ns), 0) FROM interruption_events
             WHERE interruption_type='agent_crash' AND detail LIKE '%\"oom\":true%'",
            [],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0)
    }

    /// Deduplication check: return true if a row with same call_id + type already exists.
    pub fn exists_for_call(&self, call_id: &str, itype: &InterruptionType) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM interruption_events WHERE call_id=?1 AND interruption_type=?2",
            params![call_id, itype.as_str()],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0) > 0
    }

    /// Deduplication check: return true if an unresolved row with same
    /// trace_id (conversation) + interruption_type + error message already exists.
    ///
    /// When `error_msg` is Some, uses keyword-based matching: the error is
    /// normalized to a core message (stripping nested JSON wrappers) and
    /// compared via substring containment.  This handles cases where the same
    /// error appears as a clean message in one call and as raw JSON in another.
    /// When `error_msg` is None, any unresolved row with same (trace_id, type) matches.
    pub fn exists_for_trace(&self, trace_id: &str, itype: &InterruptionType, error_msg: Option<&str>) -> bool {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT detail FROM interruption_events
             WHERE trace_id=?1 AND interruption_type=?2 AND resolved=0",
        ) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let rows = match stmt.query_map(params![trace_id, itype.as_str()], |row| {
            row.get::<_, Option<String>>(0)
        }) {
            Ok(r) => r,
            Err(_) => return false,
        };

        for detail_opt in rows.flatten() {
            match error_msg {
                None => {
                    // No error_msg filter — any existing row is a duplicate
                    return true;
                }
                Some(target) => {
                    // Compare normalized error keys (handles nested JSON vs clean message)
                    if let Some(ref detail_str) = detail_opt {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(detail_str) {
                            let stored_error = v.get("error").and_then(|e| e.as_str()).unwrap_or("");
                            if errors_match(stored_error, target) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Mark an interruption as resolved.
    pub fn resolve(&self, interruption_id: &str) -> Result<bool, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE interruption_events SET resolved=1 WHERE interruption_id=?1",
            params![interruption_id],
        )?;
        Ok(updated > 0)
    }

    // ─── Query ──────────────────────────────────────────────────────────────

    /// List interruptions within a time range.
    pub fn list(
        &self,
        start_ns: i64,
        end_ns: i64,
        agent_name: Option<&str>,
        itype: Option<&str>,
        severity: Option<&str>,
        resolved: Option<bool>,
        limit: i64,
    ) -> Result<Vec<InterruptionRecord>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();

        // Build dynamic WHERE clause
        let mut conditions = vec![
            "occurred_at_ns BETWEEN ?1 AND ?2".to_string(),
        ];
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(start_ns),
            Box::new(end_ns),
        ];
        let mut idx = 3usize;

        if let Some(a) = agent_name {
            conditions.push(format!("agent_name = ?{}", idx));
            args.push(Box::new(a.to_string()));
            idx += 1;
        }
        if let Some(t) = itype {
            conditions.push(format!("interruption_type = ?{}", idx));
            args.push(Box::new(t.to_string()));
            idx += 1;
        }
        if let Some(s) = severity {
            conditions.push(format!("severity = ?{}", idx));
            args.push(Box::new(s.to_string()));
            idx += 1;
        }
        if let Some(r) = resolved {
            conditions.push(format!("resolved = ?{}", idx));
            args.push(Box::new(r as i32));
            idx += 1;
        }
        let _ = idx;

        let sql = format!(
            "SELECT id, interruption_id, session_id, trace_id, call_id, pid, agent_name,
                    interruption_type, severity, occurred_at_ns, detail, resolved
             FROM interruption_events
             WHERE {}
             ORDER BY occurred_at_ns DESC
             LIMIT ?{}",
            conditions.join(" AND "),
            args.len() + 1,
        );
        args.push(Box::new(limit));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(InterruptionRecord {
                id: row.get(0)?,
                interruption_id: row.get(1)?,
                session_id: row.get(2)?,
                trace_id: row.get(3)?,
                call_id: row.get(4)?,
                pid: row.get(5)?,
                agent_name: row.get(6)?,
                interruption_type: row.get(7)?,
                severity: row.get(8)?,
                occurred_at_ns: row.get(9)?,
                detail: row.get(10)?,
                resolved: row.get::<_, i32>(11)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows { result.push(row?); }
        Ok(result)
    }

    /// Get a single interruption by ID.
    pub fn get_by_id(&self, interruption_id: &str) -> Result<Option<InterruptionRecord>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, interruption_id, session_id, trace_id, call_id, pid, agent_name,
                    interruption_type, severity, occurred_at_ns, detail, resolved
             FROM interruption_events WHERE interruption_id=?1",
        )?;
        let mut rows = stmt.query_map(params![interruption_id], |row| {
            Ok(InterruptionRecord {
                id: row.get(0)?,
                interruption_id: row.get(1)?,
                session_id: row.get(2)?,
                trace_id: row.get(3)?,
                call_id: row.get(4)?,
                pid: row.get(5)?,
                agent_name: row.get(6)?,
                interruption_type: row.get(7)?,
                severity: row.get(8)?,
                occurred_at_ns: row.get(9)?,
                detail: row.get(10)?,
                resolved: row.get::<_, i32>(11)? != 0,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Get all interruptions for a session.
    pub fn list_by_session(&self, session_id: &str) -> Result<Vec<InterruptionRecord>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, interruption_id, session_id, trace_id, call_id, pid, agent_name,
                    interruption_type, severity, occurred_at_ns, detail, resolved
             FROM interruption_events
             WHERE session_id=?1
             ORDER BY occurred_at_ns ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(InterruptionRecord {
                id: row.get(0)?,
                interruption_id: row.get(1)?,
                session_id: row.get(2)?,
                trace_id: row.get(3)?,
                call_id: row.get(4)?,
                pid: row.get(5)?,
                agent_name: row.get(6)?,
                interruption_type: row.get(7)?,
                severity: row.get(8)?,
                occurred_at_ns: row.get(9)?,
                detail: row.get(10)?,
                resolved: row.get::<_, i32>(11)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows { result.push(row?); }
        Ok(result)
    }

    pub fn list_by_trace(&self, trace_id: &str) -> Result<Vec<InterruptionRecord>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, interruption_id, session_id, trace_id, call_id, pid, agent_name,
                    interruption_type, severity, occurred_at_ns, detail, resolved
             FROM interruption_events
             WHERE trace_id=?1
             ORDER BY occurred_at_ns ASC",
        )?;
        let rows = stmt.query_map(params![trace_id], |row| {
            Ok(InterruptionRecord {
                id: row.get(0)?,
                interruption_id: row.get(1)?,
                session_id: row.get(2)?,
                trace_id: row.get(3)?,
                call_id: row.get(4)?,
                pid: row.get(5)?,
                agent_name: row.get(6)?,
                interruption_type: row.get(7)?,
                severity: row.get(8)?,
                occurred_at_ns: row.get(9)?,
                detail: row.get(10)?,
                resolved: row.get::<_, i32>(11)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows { result.push(row?); }
        Ok(result)
    }

    /// Statistics: count by type within a time range.
    pub fn stats(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<InterruptionTypeStat>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT interruption_type, severity, COUNT(*) AS cnt
             FROM interruption_events
             WHERE occurred_at_ns BETWEEN ?1 AND ?2
             GROUP BY interruption_type
             ORDER BY cnt DESC",
        )?;
        let rows = stmt.query_map(params![start_ns, end_ns], |row| {
            Ok(InterruptionTypeStat {
                interruption_type: row.get(0)?,
                severity: row.get(1)?,
                count: row.get(2)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows { result.push(row?); }
        Ok(result)
    }

    /// Count unresolved interruptions grouped by (session_id, severity, type).
    /// Returns detailed rows for building per-severity badges with type tooltips.
    pub fn count_unresolved_by_session_detailed(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<(String, String, String, i64)>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, severity, interruption_type, COUNT(*) AS cnt
             FROM interruption_events
             WHERE session_id IS NOT NULL
               AND resolved = 0
               AND occurred_at_ns BETWEEN ?1 AND ?2
             GROUP BY session_id, severity, interruption_type
             ORDER BY session_id, cnt DESC",
        )?;
        let rows = stmt.query_map(params![start_ns, end_ns], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows { result.push(row?); }
        Ok(result)
    }

    /// Count unresolved interruptions grouped by (trace_id, severity, type).
    pub fn count_unresolved_by_trace_detailed(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<(String, String, String, i64)>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT trace_id, severity, interruption_type, COUNT(*) AS cnt
             FROM interruption_events
             WHERE trace_id IS NOT NULL
               AND resolved = 0
               AND occurred_at_ns BETWEEN ?1 AND ?2
             GROUP BY trace_id, severity, interruption_type
             ORDER BY trace_id, cnt DESC",
        )?;
        let rows = stmt.query_map(params![start_ns, end_ns], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows { result.push(row?); }
        Ok(result)
    }

    /// Purge interruption events older than cutoff_ns.
    pub fn purge_before(&self, cutoff_ns: i64) -> Result<usize, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM interruption_events WHERE occurred_at_ns < ?1",
            params![cutoff_ns],
        )?;
        Ok(deleted)
    }
}

// ─── Error matching helpers ────────────────────────────────────────────────────

/// Try to extract a clean message from a string that may contain nested JSON.
/// Returns the extracted message lowercased, or the original string lowercased.
fn normalize_error_key(raw: &str) -> String {
    let trimmed = raw.trim();
    // Try to parse as JSON and extract "message" field
    if let Some(msg) = extract_message_from_json(trimmed) {
        return msg.to_lowercase();
    }
    // Try to find JSON embedded in the string (e.g. "curl...{\"error\":{...}}")
    if let Some(brace_start) = trimmed.find('{') {
        if let Some(msg) = extract_message_from_json(&trimmed[brace_start..]) {
            return msg.to_lowercase();
        }
    }
    trimmed.to_lowercase()
}

/// Attempt to parse JSON and find a "message" field at common locations:
/// - top-level "message"
/// - "error.message"
fn extract_message_from_json(s: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    // Try "error.message"
    if let Some(msg) = v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return Some(msg.to_string());
    }
    // Try top-level "message"
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        return Some(msg.to_string());
    }
    None
}

/// Check whether two error strings refer to the same underlying error.
/// Uses keyword normalization + substring containment.
fn errors_match(a: &str, b: &str) -> bool {
    let na = normalize_error_key(a);
    let nb = normalize_error_key(b);
    if na == nb {
        return true;
    }
    // Substring containment: if one fully contains the other
    na.contains(&nb) || nb.contains(&na)
}
