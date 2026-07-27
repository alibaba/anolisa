//! SQLite storage for interruption_events table.

use rusqlite::{Connection, params};
use std::sync::Mutex;

use super::connection::create_connection;
use crate::interruption::{InterruptionEvent, InterruptionType};

// ─── API response types ────────────────────────────────────────────────────────

/// Summary returned by GET /api/interruptions
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterruptionRecord {
    pub id: i64,
    pub interruption_id: String,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub conversation_id: Option<String>,
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
        let store = InterruptionStore {
            conn: Mutex::new(conn),
        };
        store.init_tables()?;
        Ok(store)
    }

    fn init_tables(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS interruption_events (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                interruption_id     TEXT NOT NULL UNIQUE,
                session_id          TEXT,
                trace_id            TEXT,
                conversation_id     TEXT,
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
            CREATE INDEX IF NOT EXISTS idx_interruption_resolved ON interruption_events(resolved);
            CREATE INDEX IF NOT EXISTS idx_interruption_conversation ON interruption_events(conversation_id);",
        )?;
        // Migration: add conversation_id column for existing databases
        let _ =
            conn.execute_batch("ALTER TABLE interruption_events ADD COLUMN conversation_id TEXT;");
        Ok(())
    }

    // ─── Write ──────────────────────────────────────────────────────────────

    /// Insert a single interruption event (ignores duplicates by interruption_id).
    pub fn insert(&self, event: &InterruptionEvent) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR IGNORE INTO interruption_events (
                interruption_id, session_id, trace_id, conversation_id, call_id, pid, agent_name,
                interruption_type, severity, occurred_at_ns, detail, resolved
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                event.interruption_id,
                event.session_id,
                event.trace_id,
                event.conversation_id,
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
    pub fn insert_batch(
        &self,
        events: &[InterruptionEvent],
    ) -> Result<(), Box<dyn std::error::Error>> {
        for e in events {
            self.insert(e)?;
        }
        Ok(())
    }

    /// Deduplication check for OOM events: return true if an agent_crash row
    /// with oom=true already exists for the given (pid, occurred_at_ns).
    pub fn oom_event_exists(&self, pid: i32, occurred_at_ns: i64) -> bool {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT COUNT(*) FROM interruption_events
             WHERE interruption_type='agent_crash' AND pid=?1 AND occurred_at_ns=?2
               AND detail LIKE '%\"oom\":true%'",
            params![pid, occurred_at_ns],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    }

    /// Return the maximum occurred_at_ns of OOM-sourced agent_crash events.
    /// Returns 0 if no such events exist.
    pub fn latest_oom_event_ns(&self) -> i64 {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT COALESCE(MAX(occurred_at_ns), 0) FROM interruption_events
             WHERE interruption_type='agent_crash' AND detail LIKE '%\"oom\":true%'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
    }

    /// Deduplication check: return true if a row with same call_id + type already exists.
    pub fn exists_for_call(&self, call_id: &str, itype: &InterruptionType) -> bool {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT COUNT(*) FROM interruption_events WHERE call_id=?1 AND interruption_type=?2",
            params![call_id, itype.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    }

    /// Deduplication check: return true if an unresolved row with same
    /// conversation_id + interruption_type + error message already exists.
    ///
    /// When `error_msg` is Some, uses keyword-based matching: the error is
    /// normalized to a core message (stripping nested JSON wrappers) and
    /// compared via substring containment.  This handles cases where the same
    /// error appears as a clean message in one call and as raw JSON in another.
    /// When `error_msg` is None, any unresolved row with same (conversation_id, type) matches.
    pub fn exists_for_conversation(
        &self,
        conversation_id: &str,
        itype: &InterruptionType,
        error_msg: Option<&str>,
    ) -> bool {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = match conn.prepare(
            "SELECT detail FROM interruption_events
             WHERE conversation_id=?1 AND interruption_type=?2 AND resolved=0",
        ) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let rows = match stmt.query_map(params![conversation_id, itype.as_str()], |row| {
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
                            let stored_error =
                                v.get("error").and_then(|e| e.as_str()).unwrap_or("");
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
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let updated = conn.execute(
            "UPDATE interruption_events SET resolved=1 WHERE interruption_id=?1",
            params![interruption_id],
        )?;
        Ok(updated > 0)
    }

    /// Count unresolved interruptions of a given type for a conversation.
    ///
    /// Used by the DeadLoop auto-kill feature to determine whether the kill
    /// threshold has been reached.
    pub fn count_for_conversation(&self, conversation_id: &str, itype: &InterruptionType) -> usize {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result: Result<i64, _> = conn.query_row(
            "SELECT COUNT(*) FROM interruption_events
             WHERE conversation_id=?1 AND interruption_type=?2 AND resolved=0",
            params![conversation_id, itype.as_str()],
            |row| row.get(0),
        );
        result.unwrap_or(0) as usize
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
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Build dynamic WHERE clause
        let mut conditions = vec!["occurred_at_ns BETWEEN ?1 AND ?2".to_string()];
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(start_ns), Box::new(end_ns)];
        let mut idx = 3usize;

        if let Some(a) = agent_name {
            conditions.push(format!("agent_name = ?{idx}"));
            args.push(Box::new(a.to_string()));
            idx += 1;
        }
        if let Some(t) = itype {
            conditions.push(format!("interruption_type = ?{idx}"));
            args.push(Box::new(t.to_string()));
            idx += 1;
        }
        if let Some(s) = severity {
            conditions.push(format!("severity = ?{idx}"));
            args.push(Box::new(s.to_string()));
            idx += 1;
        }
        if let Some(r) = resolved {
            conditions.push(format!("resolved = ?{idx}"));
            args.push(Box::new(r as i32));
            idx += 1;
        }
        let _ = idx;

        let sql = format!(
            "SELECT id, interruption_id, session_id, trace_id, conversation_id, call_id, pid, agent_name,
                    interruption_type, severity, occurred_at_ns, detail, resolved
             FROM interruption_events
             WHERE {}
             ORDER BY occurred_at_ns DESC
             LIMIT ?{}",
            conditions.join(" AND "),
            args.len() + 1,
        );
        args.push(Box::new(limit));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            args.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(InterruptionRecord {
                id: row.get(0)?,
                interruption_id: row.get(1)?,
                session_id: row.get(2)?,
                trace_id: row.get(3)?,
                conversation_id: row.get(4)?,
                call_id: row.get(5)?,
                pid: row.get(6)?,
                agent_name: row.get(7)?,
                interruption_type: row.get(8)?,
                severity: row.get(9)?,
                occurred_at_ns: row.get(10)?,
                detail: row.get(11)?,
                resolved: row.get::<_, i32>(12)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get a single interruption by ID.
    pub fn get_by_id(
        &self,
        interruption_id: &str,
    ) -> Result<Option<InterruptionRecord>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, interruption_id, session_id, trace_id, conversation_id, call_id, pid, agent_name,
                    interruption_type, severity, occurred_at_ns, detail, resolved
             FROM interruption_events WHERE interruption_id=?1",
        )?;
        let mut rows = stmt.query_map(params![interruption_id], |row| {
            Ok(InterruptionRecord {
                id: row.get(0)?,
                interruption_id: row.get(1)?,
                session_id: row.get(2)?,
                trace_id: row.get(3)?,
                conversation_id: row.get(4)?,
                call_id: row.get(5)?,
                pid: row.get(6)?,
                agent_name: row.get(7)?,
                interruption_type: row.get(8)?,
                severity: row.get(9)?,
                occurred_at_ns: row.get(10)?,
                detail: row.get(11)?,
                resolved: row.get::<_, i32>(12)? != 0,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Get all interruptions for a session.
    pub fn list_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<InterruptionRecord>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, interruption_id, session_id, trace_id, conversation_id, call_id, pid, agent_name,
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
                conversation_id: row.get(4)?,
                call_id: row.get(5)?,
                pid: row.get(6)?,
                agent_name: row.get(7)?,
                interruption_type: row.get(8)?,
                severity: row.get(9)?,
                occurred_at_ns: row.get(10)?,
                detail: row.get(11)?,
                resolved: row.get::<_, i32>(12)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn list_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<InterruptionRecord>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, interruption_id, session_id, trace_id, conversation_id, call_id, pid, agent_name,
                    interruption_type, severity, occurred_at_ns, detail, resolved
             FROM interruption_events
             WHERE conversation_id=?1
             ORDER BY occurred_at_ns ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok(InterruptionRecord {
                id: row.get(0)?,
                interruption_id: row.get(1)?,
                session_id: row.get(2)?,
                trace_id: row.get(3)?,
                conversation_id: row.get(4)?,
                call_id: row.get(5)?,
                pid: row.get(6)?,
                agent_name: row.get(7)?,
                interruption_type: row.get(8)?,
                severity: row.get(9)?,
                occurred_at_ns: row.get(10)?,
                detail: row.get(11)?,
                resolved: row.get::<_, i32>(12)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Statistics: count by type within a time range.
    pub fn stats(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<InterruptionTypeStat>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
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
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Count unresolved interruptions grouped by (session_id, severity, type).
    /// Returns detailed rows for building per-severity badges with type tooltips.
    pub fn count_unresolved_by_session_detailed(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<(String, String, String, i64)>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
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
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Count unresolved interruptions grouped by (conversation_id, severity, type).
    pub fn count_unresolved_by_conversation_detailed(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<(String, String, String, i64)>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT conversation_id, severity, interruption_type, COUNT(*) AS cnt
             FROM interruption_events
             WHERE conversation_id IS NOT NULL
               AND resolved = 0
               AND occurred_at_ns BETWEEN ?1 AND ?2
             GROUP BY conversation_id, severity, interruption_type
             ORDER BY conversation_id, cnt DESC",
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
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Check if a recent agent_crash event exists for the given PID.
    ///
    /// Used for dedup between trace-mode crash detection and serve-mode
    /// HealthChecker: if trace already recorded the crash, serve skips it.
    pub fn agent_crash_exists_recent(&self, pid: i32, window_secs: u64) -> bool {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let cutoff_ns = now_ns - (window_secs as i64 * 1_000_000_000);
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT COUNT(*) FROM interruption_events
             WHERE interruption_type='agent_crash' AND pid=?1 AND occurred_at_ns > ?2",
            params![pid, cutoff_ns],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    }

    /// Purge interruption events older than cutoff_ns.
    pub fn purge_before(&self, cutoff_ns: i64) -> Result<usize, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let deleted = conn.execute(
            "DELETE FROM interruption_events WHERE occurred_at_ns < ?1",
            params![cutoff_ns],
        )?;
        Ok(deleted)
    }

    /// Purge old records and trim the DB file if it exceeds a size budget.
    ///
    /// * `retention_days` - delete rows whose `occurred_at_ns` is older than this
    ///   many days.  A value of 0 disables age-based purging.
    /// * `max_db_size_mb` - if the database file is larger than this, delete the
    ///   oldest rows until it fits.  A value of 0 disables size-based purging.
    ///
    /// Returns the total number of rows deleted.
    pub fn purge_old_and_oversized(
        &self,
        retention_days: u64,
        max_db_size_mb: u64,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let mut total_deleted = 0usize;

        // 1. Age-based retention
        if retention_days > 0 {
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            let retention_ns = retention_days as i64 * 24 * 3600 * 1_000_000_000;
            let cutoff_ns = now_ns.saturating_sub(retention_ns);
            total_deleted += self.purge_before(cutoff_ns)?;
        }

        // 2. Size-based trimming: if file still exceeds max_db_size_mb, delete
        // oldest records in batches until it fits or no rows remain.
        if max_db_size_mb > 0 {
            let max_bytes = (max_db_size_mb as u64) * 1024 * 1024;
            let path = self.db_path();
            for _ in 0..100 {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if size <= max_bytes {
                    break;
                }
                let deleted = self.delete_oldest_batch(1000)?;
                total_deleted += deleted;
                if deleted == 0 {
                    break;
                }
            }
            // Reclaim free pages after bulk deletes.
            let _ = self.vacuum();
        }

        Ok(total_deleted)
    }

    /// Delete the oldest N interruption events.
    fn delete_oldest_batch(&self, limit: usize) -> Result<usize, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let deleted = conn.execute(
            "DELETE FROM interruption_events WHERE id IN (
                SELECT id FROM interruption_events ORDER BY occurred_at_ns ASC LIMIT ?1
            )",
            params![limit as i64],
        )?;
        Ok(deleted)
    }

    /// Return the filesystem path of this store.
    fn db_path(&self) -> std::path::PathBuf {
        // The connection was created from a path in `new_with_path`; we can
        // recover it via `path()` on the underlying connection.
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.path()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("interruption_events.db"))
    }

    /// Run VACUUM to reclaim free pages.
    fn vacuum(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute_batch("VACUUM;")?;
        Ok(())
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
    if let Some(msg) = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store() -> InterruptionStore {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "test_interruption_{}.db",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        InterruptionStore::new_with_path(&path).unwrap()
    }

    fn make_event(conversation_id: &str, itype: InterruptionType) -> InterruptionEvent {
        InterruptionEvent {
            interruption_id: format!(
                "int-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ),
            session_id: Some("sess-1".to_string()),
            trace_id: None,
            conversation_id: Some(conversation_id.to_string()),
            call_id: None,
            pid: Some(1234),
            agent_name: Some("TestAgent".to_string()),
            interruption_type: itype,
            severity: crate::interruption::types::Severity::Critical,
            occurred_at_ns: 1000000000,
            detail: None,
            resolved: false,
        }
    }

    #[test]
    fn test_count_for_conversation_empty() {
        let store = temp_store();
        let count = store.count_for_conversation("conv-999", &InterruptionType::DeadLoop);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_count_for_conversation_single() {
        let store = temp_store();
        let event = make_event("conv-1", InterruptionType::DeadLoop);
        store.insert(&event).unwrap();
        let count = store.count_for_conversation("conv-1", &InterruptionType::DeadLoop);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_count_for_conversation_multiple() {
        let store = temp_store();
        for i in 0..3 {
            let mut event = make_event("conv-2", InterruptionType::DeadLoop);
            event.interruption_id = format!("int-multi-{i}");
            store.insert(&event).unwrap();
        }
        let count = store.count_for_conversation("conv-2", &InterruptionType::DeadLoop);
        assert_eq!(count, 3);
    }

    #[test]
    fn test_count_for_conversation_different_type_not_counted() {
        let store = temp_store();
        let event = make_event("conv-3", InterruptionType::RetryStorm);
        store.insert(&event).unwrap();
        let count = store.count_for_conversation("conv-3", &InterruptionType::DeadLoop);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_count_for_conversation_different_conv_not_counted() {
        let store = temp_store();
        let mut event = make_event("conv-4", InterruptionType::DeadLoop);
        event.interruption_id = "int-other-conv".to_string();
        store.insert(&event).unwrap();
        let count = store.count_for_conversation("conv-5", &InterruptionType::DeadLoop);
        assert_eq!(count, 0);
    }

    /// After intentionally poisoning the conn mutex, methods that use
    /// `unwrap_or_else(|e| e.into_inner())` should still operate correctly.
    #[test]
    fn poison_recovery_conn_still_operational() {
        let store = temp_store();

        // Intentionally poison the conn mutex
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = store.conn.lock().unwrap();
            panic!("intentional poison");
        }));
        assert!(result.is_err(), "Mutex should be poisoned");

        // Exercise the poison-recovery path via count_for_conversation
        // which locks conn via unwrap_or_else(|e| e.into_inner())
        let count = store.count_for_conversation("conv-none", &InterruptionType::DeadLoop);
        assert_eq!(count, 0, "Should still query after poison recovery");

        // Also exercise a write path (insert)
        let event = make_event("conv-poison", InterruptionType::DeadLoop);
        store.insert(&event).unwrap();
    }

    // ── insert / get_by_id ──────────────────────────────────────────────────

    #[test]
    fn insert_and_get_by_id() {
        let store = temp_store();
        let mut event = make_event("conv-ins", InterruptionType::LlmError);
        event.interruption_id = "int-get-1".to_string();
        event.detail = Some(r#"{"error":"bad request"}"#.to_string());
        store.insert(&event).unwrap();

        let row = store
            .get_by_id("int-get-1")
            .unwrap()
            .expect("row should exist");
        assert_eq!(row.interruption_id, "int-get-1");
        assert_eq!(row.interruption_type, "llm_error");
        assert_eq!(row.severity, "critical");
        assert!(!row.resolved);
        assert_eq!(row.detail.as_deref(), Some(r#"{"error":"bad request"}"#));
    }

    #[test]
    fn insert_duplicate_ignored() {
        let store = temp_store();
        let event = make_event("conv-dup", InterruptionType::RateLimit);
        // First insert
        store.insert(&event).unwrap();
        // Second insert with same interruption_id is silently ignored
        store.insert(&event).unwrap();

        let records = store
            .list(0, i64::MAX, None, None, None, None, 100)
            .unwrap();
        assert_eq!(records.len(), 1, "duplicate should be ignored");
    }

    #[test]
    fn insert_batch_inserts_multiple() {
        let store = temp_store();
        let events: Vec<_> = (0..3)
            .map(|i| {
                let mut e = make_event("conv-batch", InterruptionType::SseTruncated);
                e.interruption_id = format!("int-batch-{i}");
                e
            })
            .collect();
        store.insert_batch(&events).unwrap();

        let records = store
            .list(0, i64::MAX, None, None, None, None, 100)
            .unwrap();
        assert_eq!(records.len(), 3);
    }

    // ── resolve ──────────────────────────────────────────────────────────────

    #[test]
    fn resolve_existing_event() {
        let store = temp_store();
        let mut event = make_event("conv-resolve", InterruptionType::AuthError);
        event.interruption_id = "int-resolve-1".to_string();
        store.insert(&event).unwrap();

        assert!(store.resolve("int-resolve-1").unwrap());

        let row = store.get_by_id("int-resolve-1").unwrap().unwrap();
        assert!(row.resolved);
    }

    #[test]
    fn resolve_nonexistent_returns_false() {
        let store = temp_store();
        assert!(!store.resolve("no-such-id").unwrap());
    }

    // ── list with filters ────────────────────────────────────────────────────

    #[test]
    fn list_filters_by_agent_name_type_severity_resolved() {
        let store = temp_store();
        let mut e1 = make_event("conv-f1", InterruptionType::RateLimit);
        e1.interruption_id = "int-f1".to_string();
        e1.agent_name = Some("Agent-A".to_string());
        e1.severity = crate::interruption::types::Severity::Medium;
        store.insert(&e1).unwrap();

        let mut e2 = make_event("conv-f2", InterruptionType::AuthError);
        e2.interruption_id = "int-f2".to_string();
        e2.agent_name = Some("Agent-B".to_string());
        e2.severity = crate::interruption::types::Severity::High;
        store.insert(&e2).unwrap();

        // Filter by agent_name
        let rows = store
            .list(0, i64::MAX, Some("Agent-A"), None, None, None, 100)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interruption_id, "int-f1");

        // Filter by type
        let rows = store
            .list(0, i64::MAX, None, Some("auth_error"), None, None, 100)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interruption_id, "int-f2");

        // Filter by severity
        let rows = store
            .list(0, i64::MAX, None, None, Some("medium"), None, 100)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interruption_id, "int-f1");

        // Filter by resolved=false (both unresolved)
        let rows = store
            .list(0, i64::MAX, None, None, None, Some(false), 100)
            .unwrap();
        assert_eq!(rows.len(), 2);

        // Resolve one, then filter resolved=true
        store.resolve("int-f1").unwrap();
        let rows = store
            .list(0, i64::MAX, None, None, None, Some(true), 100)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interruption_id, "int-f1");
    }

    #[test]
    fn list_respects_time_range_and_limit() {
        let store = temp_store();
        for i in 0..5 {
            let mut e = make_event("conv-tr", InterruptionType::LlmError);
            e.interruption_id = format!("int-tr-{i}");
            e.occurred_at_ns = 1_000_000_000 + i * 1_000_000_000;
            store.insert(&e).unwrap();
        }

        // Time range: only events at 2B and 3B ns
        let rows = store
            .list(2_000_000_000, 3_000_000_000, None, None, None, None, 100)
            .unwrap();
        assert_eq!(rows.len(), 2);

        // Limit 2 (most recent first)
        let rows = store.list(0, i64::MAX, None, None, None, None, 2).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows[0].occurred_at_ns >= rows[1].occurred_at_ns,
            "should be DESC order"
        );
    }

    // ── list_by_session / list_by_conversation ───────────────────────────────

    #[test]
    fn list_by_session_returns_matching_rows() {
        let store = temp_store();
        let mut e1 = make_event("conv-ls", InterruptionType::DeadLoop);
        e1.interruption_id = "int-ls-1".to_string();
        e1.session_id = Some("sess-A".to_string());
        store.insert(&e1).unwrap();

        let mut e2 = make_event("conv-ls2", InterruptionType::RetryStorm);
        e2.interruption_id = "int-ls-2".to_string();
        e2.session_id = Some("sess-B".to_string());
        store.insert(&e2).unwrap();

        let rows = store.list_by_session("sess-A").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interruption_id, "int-ls-1");
    }

    #[test]
    fn list_by_conversation_returns_matching_rows() {
        let store = temp_store();
        let mut e1 = make_event("conv-lc", InterruptionType::TokenLimit);
        e1.interruption_id = "int-lc-1".to_string();
        store.insert(&e1).unwrap();

        let rows = store.list_by_conversation("conv-lc").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].conversation_id.as_deref(), Some("conv-lc"));
    }

    // ── stats ────────────────────────────────────────────────────────────────

    #[test]
    fn stats_groups_by_type() {
        let store = temp_store();
        for i in 0..2 {
            let mut e = make_event("conv-st", InterruptionType::RateLimit);
            e.interruption_id = format!("int-st-rl-{i}");
            e.occurred_at_ns = 5_000_000_000;
            store.insert(&e).unwrap();
        }
        let mut e = make_event("conv-st2", InterruptionType::AgentCrash);
        e.interruption_id = "int-st-crash".to_string();
        e.occurred_at_ns = 5_000_000_000;
        store.insert(&e).unwrap();

        let stats = store.stats(0, i64::MAX).unwrap();
        // rate_limit should have count=2, agent_crash count=1
        let rl = stats
            .iter()
            .find(|s| s.interruption_type == "rate_limit")
            .unwrap();
        assert_eq!(rl.count, 2);
        let crash = stats
            .iter()
            .find(|s| s.interruption_type == "agent_crash")
            .unwrap();
        assert_eq!(crash.count, 1);
    }

    // ── count_unresolved_by_session/conversation_detailed ─────────────────────

    #[test]
    fn count_unresolved_by_session_detailed_groups_correctly() {
        let store = temp_store();
        let mut e1 = make_event("conv-csd1", InterruptionType::DeadLoop);
        e1.interruption_id = "int-csd-1".to_string();
        e1.session_id = Some("sess-X".to_string());
        store.insert(&e1).unwrap();

        // Resolved event should be excluded
        let mut e2 = make_event("conv-csd2", InterruptionType::DeadLoop);
        e2.interruption_id = "int-csd-2".to_string();
        e2.session_id = Some("sess-X".to_string());
        store.insert(&e2).unwrap();
        store.resolve("int-csd-2").unwrap();

        let rows = store
            .count_unresolved_by_session_detailed(0, i64::MAX)
            .unwrap();
        assert_eq!(rows.len(), 1, "only unresolved rows should appear");
        assert_eq!(rows[0].0, "sess-X");
        assert_eq!(rows[0].3, 1, "count should be 1 (resolved excluded)");
    }

    #[test]
    fn count_unresolved_by_conversation_detailed_groups_correctly() {
        let store = temp_store();
        let mut e = make_event("conv-ccd", InterruptionType::LlmError);
        e.interruption_id = "int-ccd-1".to_string();
        store.insert(&e).unwrap();

        let rows = store
            .count_unresolved_by_conversation_detailed(0, i64::MAX)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "conv-ccd");
    }

    // ── OOM dedup ────────────────────────────────────────────────────────────

    #[test]
    fn oom_event_exists_and_latest_oom_event_ns() {
        let store = temp_store();
        let mut event = make_event("conv-oom", InterruptionType::AgentCrash);
        event.interruption_id = "int-oom-1".to_string();
        event.pid = Some(9999);
        event.occurred_at_ns = 7_000_000_000;
        event.detail = Some(r#"{"oom":true,"reason":"out of memory"}"#.to_string());
        store.insert(&event).unwrap();

        assert!(store.oom_event_exists(9999, 7_000_000_000));
        assert!(!store.oom_event_exists(9999, 8_000_000_000));
        assert!(!store.oom_event_exists(1111, 7_000_000_000));
        assert_eq!(store.latest_oom_event_ns(), 7_000_000_000);
    }

    #[test]
    fn latest_oom_event_ns_returns_zero_when_no_oom() {
        let store = temp_store();
        assert_eq!(store.latest_oom_event_ns(), 0);
    }

    // ── exists_for_call ──────────────────────────────────────────────────────

    #[test]
    fn exists_for_call_dedup() {
        let store = temp_store();
        let mut event = make_event("conv-efc", InterruptionType::SseTruncated);
        event.interruption_id = "int-efc-1".to_string();
        event.call_id = Some("call-abc".to_string());
        store.insert(&event).unwrap();

        assert!(store.exists_for_call("call-abc", &InterruptionType::SseTruncated));
        assert!(!store.exists_for_call("call-abc", &InterruptionType::RateLimit));
        assert!(!store.exists_for_call("call-xyz", &InterruptionType::SseTruncated));
    }

    // ── exists_for_conversation ──────────────────────────────────────────────

    #[test]
    fn exists_for_conversation_no_filter_matches_any() {
        let store = temp_store();
        let mut e = make_event("conv-efn", InterruptionType::RetryStorm);
        e.interruption_id = "int-efn-1".to_string();
        store.insert(&e).unwrap();

        // No error_msg filter: any unresolved row with same conv+type matches
        assert!(store.exists_for_conversation("conv-efn", &InterruptionType::RetryStorm, None));
        assert!(!store.exists_for_conversation("conv-efn", &InterruptionType::DeadLoop, None));
    }

    #[test]
    fn exists_for_conversation_resolved_not_matched() {
        let store = temp_store();
        let mut e = make_event("conv-efr", InterruptionType::LlmError);
        e.interruption_id = "int-efr-1".to_string();
        store.insert(&e).unwrap();
        store.resolve("int-efr-1").unwrap();

        // Resolved rows should not match (resolved=0 filter in SQL)
        assert!(!store.exists_for_conversation("conv-efr", &InterruptionType::LlmError, None));
    }

    #[test]
    fn exists_for_conversation_json_error_matching() {
        let store = temp_store();
        let mut e = make_event("conv-efj", InterruptionType::LlmError);
        e.interruption_id = "int-efj-1".to_string();
        e.detail = Some(r#"{"error":"rate limit exceeded"}"#.to_string());
        store.insert(&e).unwrap();

        // Same error substring should match
        assert!(store.exists_for_conversation(
            "conv-efj",
            &InterruptionType::LlmError,
            Some("rate limit exceeded")
        ));
        // Unrelated error should not match
        assert!(!store.exists_for_conversation(
            "conv-efj",
            &InterruptionType::LlmError,
            Some("completely different error")
        ));
    }

    // ── agent_crash_exists_recent ────────────────────────────────────────────

    #[test]
    fn agent_crash_exists_recent_finds_recent_crash() {
        let store = temp_store();
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        let mut e = make_event("conv-acer", InterruptionType::AgentCrash);
        e.interruption_id = "int-acer-1".to_string();
        e.pid = Some(5555);
        e.occurred_at_ns = now_ns;
        store.insert(&e).unwrap();

        assert!(store.agent_crash_exists_recent(5555, 60));
        assert!(!store.agent_crash_exists_recent(6666, 60));
    }

    // ── purge ────────────────────────────────────────────────────────────────

    #[test]
    fn purge_before_deletes_old_rows() {
        let store = temp_store();
        let mut e1 = make_event("conv-pb1", InterruptionType::LlmError);
        e1.interruption_id = "int-pb-1".to_string();
        e1.occurred_at_ns = 1_000;
        store.insert(&e1).unwrap();

        let mut e2 = make_event("conv-pb2", InterruptionType::LlmError);
        e2.interruption_id = "int-pb-2".to_string();
        e2.occurred_at_ns = 10_000_000_000;
        store.insert(&e2).unwrap();

        let deleted = store.purge_before(5_000_000_000).unwrap();
        assert_eq!(deleted, 1);

        let remaining = store
            .list(0, i64::MAX, None, None, None, None, 100)
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].interruption_id, "int-pb-2");
    }

    #[test]
    fn purge_old_and_oversized_age_based() {
        let store = temp_store();
        let mut e = make_event("conv-poo", InterruptionType::LlmError);
        e.interruption_id = "int-poo-1".to_string();
        e.occurred_at_ns = 1; // very old
        store.insert(&e).unwrap();

        let deleted = store.purge_old_and_oversized(1, 0).unwrap();
        assert_eq!(
            deleted, 1,
            "very old row should be purged with 1-day retention"
        );
    }

    // ── pure helper functions ────────────────────────────────────────────────

    #[test]
    fn normalize_error_key_plain_string() {
        assert_eq!(
            normalize_error_key("Rate Limit Exceeded"),
            "rate limit exceeded"
        );
    }

    #[test]
    fn normalize_error_key_json_message() {
        let json = r#"{"error":{"message":"model overloaded"}}"#;
        assert_eq!(normalize_error_key(json), "model overloaded");
    }

    #[test]
    fn normalize_error_key_top_level_message() {
        let json = r#"{"message":"bad request"}"#;
        assert_eq!(normalize_error_key(json), "bad request");
    }

    #[test]
    fn normalize_error_key_embedded_json() {
        let raw = r#"curl error: {"error":{"message":"connection refused"}}"#;
        assert_eq!(normalize_error_key(raw), "connection refused");
    }

    #[test]
    fn extract_message_from_json_nested_error() {
        let json = r#"{"error":{"message":"nested msg"}}"#;
        assert_eq!(
            extract_message_from_json(json),
            Some("nested msg".to_string())
        );
    }

    #[test]
    fn extract_message_from_json_top_level() {
        let json = r#"{"message":"top msg"}"#;
        assert_eq!(extract_message_from_json(json), Some("top msg".to_string()));
    }

    #[test]
    fn extract_message_from_json_no_message() {
        let json = r#"{"error":"just a string"}"#;
        assert_eq!(extract_message_from_json(json), None);
    }

    #[test]
    fn extract_message_from_json_invalid() {
        assert_eq!(extract_message_from_json("not json"), None);
    }

    #[test]
    fn errors_match_identical() {
        assert!(errors_match("rate limit", "rate limit"));
    }

    #[test]
    fn errors_match_case_insensitive() {
        assert!(errors_match("Rate Limit", "rate limit"));
    }

    #[test]
    fn errors_match_substring_containment() {
        assert!(errors_match("rate limit exceeded", "rate limit"));
        assert!(errors_match("rate limit", "rate limit exceeded"));
    }

    #[test]
    fn errors_match_unrelated() {
        assert!(!errors_match("rate limit", "auth error"));
    }
}
