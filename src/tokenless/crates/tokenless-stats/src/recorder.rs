//! Statistics recorder for tokenless.
//!
//! Provides SQLite-based storage for compression and rewriting metrics.

use crate::record::{OperationType, StatsRecord};
use chrono::DateTime;
use rusqlite::Connection;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

/// Result type for stats operations
pub type StatsResult<T> = Result<T, StatsError>;

/// Error types for stats operations
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Statistics recorder that stores metrics in SQLite
pub struct StatsRecorder {
    conn: Mutex<Connection>,
}

impl StatsRecorder {
    /// Create a new recorder with database at the given path
    pub fn new<P: AsRef<Path>>(db_path: P) -> StatsResult<Self> {
        let conn = Connection::open(db_path)?;

        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA busy_timeout=5000;
            PRAGMA synchronous=NORMAL;
        ",
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                operation TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                source_pid INTEGER,
                session_id TEXT,
                tool_use_id TEXT,
                before_chars INTEGER NOT NULL,
                before_tokens INTEGER NOT NULL,
                after_chars INTEGER NOT NULL,
                after_tokens INTEGER NOT NULL,
                before_text TEXT,
                after_text TEXT,
                before_output TEXT,
                after_output TEXT
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON stats(timestamp)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_operation ON stats(operation)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_agent_id ON stats(agent_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_session_id ON stats(session_id)",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Record a statistics entry
    pub fn record(&self, record: &StatsRecord) -> StatsResult<i64> {
        let conn = self.conn.lock().map_err(|e| {
            self.conn.clear_poison();
            StatsError::Database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(format!("Lock poisoned: {}", e)),
            ))
        })?;

        conn.execute(
            "INSERT INTO stats (
                timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                before_chars, before_tokens, after_chars, after_tokens,
                before_text, after_text,
                before_output, after_output
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                record.timestamp.to_rfc3339(),
                record.operation.as_str(),
                record.agent_id,
                record.source_pid,
                record.session_id,
                record.tool_use_id,
                record.before_chars,
                record.before_tokens,
                record.after_chars,
                record.after_tokens,
                record.before_text,
                record.after_text,
                record.before_output,
                record.after_output,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Query all records, newest first, with optional limit
    pub fn all_records(&self, limit: Option<usize>) -> StatsResult<Vec<StatsRecord>> {
        let conn = self.conn.lock().map_err(|e| {
            self.conn.clear_poison();
            StatsError::Database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(format!("Lock poisoned: {}", e)),
            ))
        })?;

        let sql = match limit {
            Some(n) => format!(
                "SELECT id, timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                        before_chars, before_tokens, after_chars, after_tokens,
                        before_text, after_text, before_output, after_output
                 FROM stats ORDER BY timestamp DESC LIMIT {}",
                n
            ),
            None => String::from(
                "SELECT id, timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                        before_chars, before_tokens, after_chars, after_tokens,
                        before_text, after_text, before_output, after_output
                 FROM stats ORDER BY timestamp DESC",
            ),
        };

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::row_to_record)?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get a single record by database ID
    pub fn record_by_id(&self, id: i64) -> StatsResult<Option<StatsRecord>> {
        let conn = self.conn.lock().map_err(|e| {
            self.conn.clear_poison();
            StatsError::Database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(format!("Lock poisoned: {}", e)),
            ))
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                    before_chars, before_tokens, after_chars, after_tokens,
                    before_text, after_text, before_output, after_output
             FROM stats WHERE id = ?",
        )?;

        let mut rows = stmt.query_map([id], Self::row_to_record)?;

        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    /// Get record count
    pub fn count(&self) -> StatsResult<usize> {
        let conn = self.conn.lock().map_err(|e| {
            self.conn.clear_poison();
            StatsError::Database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(format!("Lock poisoned: {}", e)),
            ))
        })?;

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM stats", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Clear all records and reset auto-increment
    pub fn clear(&self) -> StatsResult<()> {
        let conn = self.conn.lock().map_err(|e| {
            self.conn.clear_poison();
            StatsError::Database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(format!("Lock poisoned: {}", e)),
            ))
        })?;

        conn.execute_batch("DELETE FROM stats; DELETE FROM sqlite_sequence WHERE name='stats';")?;
        Ok(())
    }

    /// Convert a database row to StatsRecord
    fn row_to_record(row: &rusqlite::Row<'_>) -> Result<StatsRecord, rusqlite::Error> {
        let agent_id: String = row.get(3)?;
        Ok(StatsRecord {
            id: row.get(0)?,
            timestamp: DateTime::parse_from_rfc3339(&row.get::<_, String>(1)?)
                .map(|dt| dt.with_timezone(&chrono::Local))
                .unwrap_or_else(|_| chrono::Local::now()),
            operation: OperationType::from_str(&row.get::<_, String>(2)?)
                .unwrap_or(OperationType::CompressSchema),
            agent_id,
            source_pid: row.get(4)?,
            session_id: row.get(5)?,
            tool_use_id: row.get(6)?,
            before_chars: row.get(7)?,
            before_tokens: row.get(8)?,
            after_chars: row.get(9)?,
            after_tokens: row.get(10)?,
            before_text: row.get(11)?,
            after_text: row.get(12)?,
            before_output: row.get(13)?,
            after_output: row.get(14)?,
        })
    }
}

/// Summary statistics
#[derive(Debug, Clone, Default)]
pub struct StatsSummary {
    pub total_records: usize,
    pub total_before_chars: usize,
    pub total_after_chars: usize,
    pub total_before_tokens: usize,
    pub total_after_tokens: usize,
}

impl StatsSummary {
    pub fn chars_saved(&self) -> usize {
        self.total_before_chars
            .saturating_sub(self.total_after_chars)
    }

    pub fn tokens_saved(&self) -> usize {
        self.total_before_tokens
            .saturating_sub(self.total_after_tokens)
    }

    pub fn chars_percent(&self) -> f64 {
        if self.total_before_chars > 0 {
            (self.chars_saved() as f64 / self.total_before_chars as f64) * 100.0
        } else {
            0.0
        }
    }

    pub fn tokens_percent(&self) -> f64 {
        if self.total_before_tokens > 0 {
            (self.tokens_saved() as f64 / self.total_before_tokens as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Build summary from a slice of records
    pub fn from_records(records: &[StatsRecord]) -> Self {
        let mut summary = Self {
            total_records: records.len(),
            ..Default::default()
        };

        for record in records {
            summary.total_before_chars += record.before_chars;
            summary.total_after_chars += record.after_chars;
            summary.total_before_tokens += record.before_tokens;
            summary.total_after_tokens += record.after_tokens;
        }

        summary
    }
}
