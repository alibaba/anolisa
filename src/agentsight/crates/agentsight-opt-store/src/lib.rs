//! SQLite persistence for optimization analysis results.
//!
//! One row per analyzed session; each analysis dimension is stored as a JSON
//! string column so the schema stays stable while dimension payloads evolve.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Errors produced by [`OptimizationStore`].
#[derive(Debug, thiserror::Error)]
pub enum OptStoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store mutex poisoned")]
    Poisoned,
}

/// Analysis dimension identifying which result column to update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Perf,
    PerfIssues,
    Cost,
    CostWaste,
    Accuracy,
}

impl Dimension {
    /// Column name backing this dimension. Static strings only — never
    /// interpolate user input into SQL.
    fn column(self) -> &'static str {
        match self {
            Dimension::Perf => "perf",
            Dimension::PerfIssues => "perf_issues",
            Dimension::Cost => "cost",
            Dimension::CostWaste => "cost_waste",
            Dimension::Accuracy => "accuracy",
        }
    }
}

/// Persisted per-session optimization results (dimension payloads are JSON strings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationRecord {
    pub session_id: String,
    pub perf: Option<String>,
    pub perf_issues: Option<String>,
    pub cost: Option<String>,
    pub cost_waste: Option<String>,
    pub accuracy: Option<String>,
    pub created_at_ns: i64,
    pub updated_at_ns: i64,
}

/// Thread-safe store over a dedicated `optimization.db`.
pub struct OptimizationStore {
    conn: Mutex<Connection>,
}

impl OptimizationStore {
    /// Opens (creating if needed) the database at `path` and ensures the schema.
    ///
    /// # Errors
    /// Returns [`OptStoreError::Sqlite`] if the database cannot be opened or
    /// the schema cannot be created.
    pub fn new_with_path(path: &Path) -> Result<Self, OptStoreError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_millis(500))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS optimization_results (
                session_id TEXT PRIMARY KEY,
                perf TEXT,
                perf_issues TEXT,
                cost TEXT,
                cost_waste TEXT,
                accuracy TEXT,
                created_at_ns INTEGER NOT NULL,
                updated_at_ns INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Upserts one dimension result (JSON string) for a session.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn save_dimension(
        &self,
        session_id: &str,
        dimension: Dimension,
        result_json: &str,
    ) -> Result<(), OptStoreError> {
        let now_ns = now_ns();
        let conn = self.conn.lock().map_err(|_| OptStoreError::Poisoned)?;
        let sql = format!(
            "INSERT INTO optimization_results (session_id, {col}, created_at_ns, updated_at_ns)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(session_id) DO UPDATE SET {col} = ?2, updated_at_ns = ?3",
            col = dimension.column()
        );
        conn.execute(&sql, params![session_id, result_json, now_ns])?;
        Ok(())
    }

    /// Fetches the stored results for a session, if any.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn get(&self, session_id: &str) -> Result<Option<OptimizationRecord>, OptStoreError> {
        let conn = self.conn.lock().map_err(|_| OptStoreError::Poisoned)?;
        let record = conn
            .query_row(
                "SELECT session_id, perf, perf_issues, cost, cost_waste, accuracy,
                        created_at_ns, updated_at_ns
                 FROM optimization_results WHERE session_id = ?1",
                params![session_id],
                Self::map_row,
            )
            .optional()?;
        Ok(record)
    }

    /// Lists records updated within `[start_ns, end_ns]`, newest first.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn list(
        &self,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
    ) -> Result<Vec<OptimizationRecord>, OptStoreError> {
        let conn = self.conn.lock().map_err(|_| OptStoreError::Poisoned)?;
        let mut stmt = conn.prepare(
            "SELECT session_id, perf, perf_issues, cost, cost_waste, accuracy,
                    created_at_ns, updated_at_ns
             FROM optimization_results
             WHERE updated_at_ns >= ?1 AND updated_at_ns <= ?2
             ORDER BY updated_at_ns DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![start_ns, end_ns, limit as i64], Self::map_row)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// Deletes records older than `cutoff_ns`, returning the number removed.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn prune_before(&self, cutoff_ns: i64) -> Result<usize, OptStoreError> {
        let conn = self.conn.lock().map_err(|_| OptStoreError::Poisoned)?;
        let removed = conn.execute(
            "DELETE FROM optimization_results WHERE updated_at_ns < ?1",
            params![cutoff_ns],
        )?;
        Ok(removed)
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OptimizationRecord> {
        Ok(OptimizationRecord {
            session_id: row.get(0)?,
            perf: row.get(1)?,
            perf_issues: row.get(2)?,
            cost: row.get(3)?,
            cost_waste: row.get(4)?,
            accuracy: row.get(5)?,
            created_at_ns: row.get(6)?,
            updated_at_ns: row.get(7)?,
        })
    }
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (OptimizationStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("opt-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test-{:?}.db", std::time::Instant::now()));
        (OptimizationStore::new_with_path(&path).unwrap(), path)
    }

    #[test]
    fn save_and_get_roundtrip() {
        let (store, _path) = temp_store();
        assert!(store.get("s1").unwrap().is_none());

        store
            .save_dimension("s1", Dimension::Perf, r#"{"total":1}"#)
            .unwrap();
        store
            .save_dimension("s1", Dimension::Accuracy, r#"{"issues":[]}"#)
            .unwrap();

        let rec = store.get("s1").unwrap().unwrap();
        assert_eq!(rec.session_id, "s1");
        assert_eq!(rec.perf.as_deref(), Some(r#"{"total":1}"#));
        assert_eq!(rec.accuracy.as_deref(), Some(r#"{"issues":[]}"#));
        assert!(rec.cost.is_none());
        assert!(rec.created_at_ns > 0);
    }

    #[test]
    fn save_all_dimensions_populates_all_columns() {
        let (store, _path) = temp_store();
        store
            .save_dimension("s1", Dimension::Perf, "perf-json")
            .unwrap();
        store
            .save_dimension("s1", Dimension::PerfIssues, "perf-issues-json")
            .unwrap();
        store
            .save_dimension("s1", Dimension::Cost, "cost-json")
            .unwrap();
        store
            .save_dimension("s1", Dimension::CostWaste, "cost-waste-json")
            .unwrap();
        store
            .save_dimension("s1", Dimension::Accuracy, "accuracy-json")
            .unwrap();

        let rec = store.get("s1").unwrap().unwrap();
        assert_eq!(rec.perf.as_deref(), Some("perf-json"));
        assert_eq!(rec.perf_issues.as_deref(), Some("perf-issues-json"));
        assert_eq!(rec.cost.as_deref(), Some("cost-json"));
        assert_eq!(rec.cost_waste.as_deref(), Some("cost-waste-json"));
        assert_eq!(rec.accuracy.as_deref(), Some("accuracy-json"));
    }

    #[test]
    fn upsert_overwrites_dimension() {
        let (store, _path) = temp_store();
        store.save_dimension("s1", Dimension::Cost, "1").unwrap();
        store.save_dimension("s1", Dimension::Cost, "2").unwrap();
        let rec = store.get("s1").unwrap().unwrap();
        assert_eq!(rec.cost.as_deref(), Some("2"));
    }

    #[test]
    fn list_filters_by_time_range() {
        let (store, _path) = temp_store();
        store.save_dimension("s1", Dimension::Perf, "{}").unwrap();
        let all = store.list(0, i64::MAX, 10).unwrap();
        assert_eq!(all.len(), 1);
        let none = store.list(0, 1, 10).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn prune_removes_old_records() {
        let (store, _path) = temp_store();
        store.save_dimension("s1", Dimension::Perf, "{}").unwrap();
        assert_eq!(store.prune_before(i64::MAX).unwrap(), 1);
        assert!(store.get("s1").unwrap().is_none());
    }
}
