//! Statistics recorder for tokenless.
//!
//! Provides SQLite-based storage for compression and rewriting metrics.

use crate::record::{CompressionMode, OperationType, StatsRecord};
use chrono::DateTime;
use rusqlite::Connection;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

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
        let conn = Connection::open(&db_path)?;
        // Restrict the stats DB to owner-only — before_text/after_text
        // columns may contain tool output with sensitive content.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(db_path.as_ref(), std::fs::Permissions::from_mode(0o600)).ok();
        }

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
                after_output TEXT,
                mode TEXT
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

        // Schema migration: add columns introduced in v0.3.0 if missing.
        // Use PRAGMA table_info to check column existence before ALTER TABLE
        // instead of relying on error-message string matching, which is
        // fragile across SQLite versions and locales.
        for col in &["before_output", "after_output", "mode"] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('stats') WHERE name = ?",
                    [col],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);
            if !exists {
                conn.execute(&format!("ALTER TABLE stats ADD COLUMN {} TEXT", col), [])?;
            }
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Acquire the connection guard, recovering from poison rather than failing.
    ///
    /// A poisoned mutex means a previous holder panicked while holding the
    /// lock. For our single-statement workload (no multi-step transactions),
    /// the SQLite connection itself remains usable — so we clear the poison
    /// and reuse the underlying guard rather than dropping the call. This
    /// keeps stats recording fail-soft after a transient panic instead of
    /// permanently breaking every subsequent query.
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| {
            eprintln!(
                "[tokenless-stats] WARNING: mutex was poisoned by a previous panic; recovering: {}",
                poisoned
            );
            self.conn.clear_poison();
            poisoned.into_inner()
        })
    }

    /// Record a statistics entry
    pub fn record(&self, record: &StatsRecord) -> StatsResult<i64> {
        let conn = self.lock_conn();

        conn.execute(
            "INSERT INTO stats (
                timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                before_chars, before_tokens, after_chars, after_tokens,
                before_text, after_text,
                before_output, after_output, mode
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
                record.mode.as_str(),
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Default limit when no limit is specified — caps memory usage
    /// from unbounded loads while remaining generous for practical use.
    const DEFAULT_LIMIT: usize = 10_000;

    /// Query all records, newest first, with optional limit
    pub fn all_records(&self, limit: Option<usize>) -> StatsResult<Vec<StatsRecord>> {
        let conn = self.lock_conn();

        const SELECT_COLS: &str =
            "id, timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                        before_chars, before_tokens, after_chars, after_tokens,
                        before_text, after_text, before_output, after_output, mode";

        let n = limit.unwrap_or(Self::DEFAULT_LIMIT);
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM stats ORDER BY timestamp DESC LIMIT ?",
            SELECT_COLS
        ))?;
        let rows = stmt.query_map([n as i64], Self::row_to_record)?;
        let records: Vec<_> = rows
            .filter_map(|r| match r {
                Ok(v) => Some(v),
                Err(e) => {
                    static CORRUPT_LOGGED: AtomicBool = AtomicBool::new(false);
                    if !CORRUPT_LOGGED.swap(true, Ordering::Relaxed) {
                        eprintln!(
                            "[tokenless-stats] skipping corrupt row(s): {} \
                             (further corrupt rows suppressed)",
                            e
                        );
                    }
                    None
                }
            })
            .collect();

        Ok(records)
    }

    /// Get a single record by database ID
    pub fn record_by_id(&self, id: i64) -> StatsResult<Option<StatsRecord>> {
        let conn = self.lock_conn();

        let mut stmt = conn.prepare(
            "SELECT id, timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                    before_chars, before_tokens, after_chars, after_tokens,
                    before_text, after_text, before_output, after_output, mode
             FROM stats WHERE id = ?",
        )?;

        let mut rows = stmt.query_map([id], Self::row_to_record)?;

        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    /// Query all records for a given session, newest first, with optional limit.
    pub fn records_by_session(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> StatsResult<Vec<StatsRecord>> {
        let conn = self.lock_conn();

        const SELECT_COLS: &str =
            "id, timestamp, operation, agent_id, source_pid, session_id, tool_use_id,
                        before_chars, before_tokens, after_chars, after_tokens,
                        before_text, after_text, before_output, after_output, mode";

        let n = limit.unwrap_or(Self::DEFAULT_LIMIT);
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM stats WHERE session_id = ? ORDER BY timestamp DESC LIMIT ?",
            SELECT_COLS
        ))?;
        let rows = stmt.query_map(rusqlite::params![session_id, n as i64], Self::row_to_record)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get record count
    pub fn count(&self) -> StatsResult<usize> {
        let conn = self.lock_conn();

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM stats", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Clear all records and reset auto-increment
    pub fn clear(&self) -> StatsResult<()> {
        let conn = self.lock_conn();

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
                .unwrap_or_else(|e| {
                    eprintln!(
                        "[tokenless-stats] corrupt timestamp, using current time: {}",
                        e
                    );
                    chrono::Local::now()
                }),
            operation: OperationType::from_str(&row.get::<_, String>(2)?).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown operation type: {}", e),
                    )),
                )
            })?,
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
            mode: CompressionMode::from_db(&row.get::<_, Option<String>>(15)?.unwrap_or_default()),
        })
    }
}

/// Summary statistics
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct StatsSummary {
    #[serde(rename = "records")]
    pub total_records: usize,
    #[serde(rename = "before_chars")]
    pub total_before_chars: usize,
    #[serde(rename = "after_chars")]
    pub total_after_chars: usize,
    #[serde(rename = "before_tokens")]
    pub total_before_tokens: usize,
    #[serde(rename = "after_tokens")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::OperationType;

    fn new_recorder() -> (StatsRecorder, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("stats.db");
        let rec = StatsRecorder::new(&db).unwrap();
        (rec, dir)
    }

    fn sample(op: OperationType, mode: CompressionMode, session: &str) -> StatsRecord {
        StatsRecord::new(op, "cli".to_string(), 1000, 400, 500, 200)
            .with_session_id(session)
            .with_mode(mode)
    }

    #[test]
    fn records_and_reads_mode() {
        let (rec, _dir) = new_recorder();
        let id = rec
            .record(&sample(
                OperationType::CompressSchema,
                CompressionMode::DryRun,
                "s1",
            ))
            .unwrap();
        let got = rec.record_by_id(id).unwrap().unwrap();
        assert_eq!(got.mode, CompressionMode::DryRun);
        assert_eq!(got.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn default_mode_is_active() {
        let (rec, _dir) = new_recorder();
        let id = rec
            .record(&sample(
                OperationType::CompressSchema,
                CompressionMode::Active,
                "s1",
            ))
            .unwrap();
        let got = rec.record_by_id(id).unwrap().unwrap();
        assert_eq!(got.mode, CompressionMode::Active);
    }

    #[test]
    fn records_by_session_filters() {
        let (rec, _dir) = new_recorder();
        rec.record(&sample(
            OperationType::CompressResponse,
            CompressionMode::Active,
            "baseline",
        ))
        .unwrap();
        rec.record(&sample(
            OperationType::CompressResponse,
            CompressionMode::DryRun,
            "tokenless",
        ))
        .unwrap();
        rec.record(&sample(
            OperationType::CompressResponse,
            CompressionMode::Active,
            "baseline",
        ))
        .unwrap();

        let baseline = rec.records_by_session("baseline", None).unwrap();
        let tokenless = rec.records_by_session("tokenless", None).unwrap();
        assert_eq!(baseline.len(), 2);
        assert_eq!(tokenless.len(), 1);
        assert_eq!(tokenless[0].mode, CompressionMode::DryRun);
    }
}
