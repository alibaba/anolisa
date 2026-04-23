//! Read-only store for ~/.tokenless/stats.db
//!
//! This database is created and maintained by an external component (tokenless).
//! AgentSight only reads from it to display token savings data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

/// Default path to the tokenless stats database
pub fn default_stats_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".tokenless").join("stats.db")
}

/// A single optimization record from stats.db
#[derive(Debug, Clone)]
pub struct TokenlessStatRow {
    pub session_id: String,
    pub tool_call_id: String,
    pub before_tokens: i64,
    pub after_tokens: i64,
    pub diff_text: Option<String>,
    pub operation: String,
}

/// Read-only store for the tokenless stats database
pub struct TokenlessStatsStore {
    conn: Connection,
}

impl TokenlessStatsStore {
    /// Open stats.db if it exists. Returns None if the file is missing.
    pub fn open_if_exists(path: &Path) -> Option<Self> {
        if !path.exists() {
            return None;
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        match Connection::open_with_flags(path, flags) {
            Ok(conn) => {
                if let Err(e) = conn.busy_timeout(Duration::from_millis(500)) {
                    log::warn!("Failed to set busy_timeout on stats.db: {}", e);
                }
                Some(TokenlessStatsStore { conn })
            }
            Err(e) => {
                log::warn!("Failed to open stats.db at {:?}: {}", path, e);
                None
            }
        }
    }

    /// Query optimization records for the given session IDs.
    ///
    /// Batches queries in groups of 500 to stay within SQLite variable limits.
    /// Returns an empty Vec on SQLITE_BUSY or other transient errors.
    pub fn get_stats_by_session_ids(&self, ids: &[&str]) -> Vec<TokenlessStatRow> {
        let mut results = Vec::new();

        for chunk in ids.chunks(500) {
            let placeholders: String = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT session_id, tool_call_id, before_tokens, after_tokens, diff_text, operation \
                 FROM stats WHERE session_id IN ({})",
                placeholders
            );

            let mut stmt = match self.conn.prepare(&sql) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("Failed to prepare stats query: {}", e);
                    return Vec::new();
                }
            };

            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();

            let rows = match stmt.query_map(params.as_slice(), |row| {
                Ok(TokenlessStatRow {
                    session_id: row.get(0)?,
                    tool_call_id: row.get(1)?,
                    before_tokens: row.get(2)?,
                    after_tokens: row.get(3)?,
                    diff_text: row.get(4)?,
                    operation: row.get(5)?,
                })
            }) {
                Ok(rows) => rows,
                Err(e) => {
                    log::warn!("Failed to query stats.db: {}", e);
                    return Vec::new();
                }
            };

            for row in rows {
                match row {
                    Ok(r) => results.push(r),
                    Err(e) => {
                        log::warn!("Error reading stats row: {}", e);
                    }
                }
            }
        }

        results
    }

    /// Group stat rows by session_id for efficient lookup.
    pub fn group_by_session(rows: Vec<TokenlessStatRow>) -> HashMap<String, Vec<TokenlessStatRow>> {
        let mut map: HashMap<String, Vec<TokenlessStatRow>> = HashMap::new();
        for row in rows {
            map.entry(row.session_id.clone()).or_default().push(row);
        }
        map
    }
}
