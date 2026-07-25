//! SQLite persistence for collected trajectories (`trajectories.db`).
//!
//! One row per session file. Queryable columns are derived from the converted
//! ATIF v1.7 document (never from the raw JSONL) so they always agree with
//! `atif_json`; bookkeeping columns (`file_*`) drive incremental scanning.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// A row of `collected_trajectories` ready for upsert.
#[derive(Debug, Clone)]
pub struct TrajectoryRecord {
    // ATIF-derived columns
    pub session_id: String,
    pub schema_version: String,
    pub agent_name: String,
    pub model_name: Option<String>,
    pub num_steps: i64,
    pub total_prompt_tokens: Option<i64>,
    pub total_completion_tokens: Option<i64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub atif_json: String,
    // Collection bookkeeping columns
    pub project: String,
    pub source: String,
    pub is_subagent: bool,
    pub file_path: String,
    pub file_size: i64,
    pub file_mtime_ns: i64,
}

/// Thread-safe store over a dedicated `trajectories.db`.
pub struct TrajectoryStore {
    conn: Mutex<Connection>,
}

impl TrajectoryStore {
    /// Opens (creating if needed) the database at `path` and ensures the schema.
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or the schema
    /// cannot be created.
    pub fn new_with_path(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("open trajectories db {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_millis(500))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS collected_trajectories (
                session_id TEXT PRIMARY KEY,
                schema_version TEXT NOT NULL,
                agent_name TEXT NOT NULL,
                model_name TEXT,
                num_steps INTEGER NOT NULL,
                total_prompt_tokens INTEGER,
                total_completion_tokens INTEGER,
                start_time TEXT,
                end_time TEXT,
                atif_json TEXT NOT NULL,
                project TEXT NOT NULL,
                source TEXT NOT NULL,
                is_subagent INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                file_mtime_ns INTEGER NOT NULL,
                collected_at_ns INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Returns `(file_size, file_mtime_ns)` recorded for `file_path`, if any.
    /// Drives the incremental scan: unchanged files are skipped.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn get_file_state(&self, file_path: &str) -> Result<Option<(i64, i64)>> {
        let conn = self.lock_conn()?;
        let state = conn
            .query_row(
                "SELECT file_size, file_mtime_ns FROM collected_trajectories
                 WHERE file_path = ?1",
                params![file_path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(state)
    }

    /// Inserts or updates one trajectory row (keyed by `session_id`).
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn upsert_trajectory(&self, record: &TrajectoryRecord) -> Result<()> {
        let now_ns = now_ns();
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO collected_trajectories (
                session_id, schema_version, agent_name, model_name, num_steps,
                total_prompt_tokens, total_completion_tokens, start_time, end_time,
                atif_json, project, source, is_subagent, file_path, file_size,
                file_mtime_ns, collected_at_ns
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(session_id) DO UPDATE SET
                schema_version = ?2, agent_name = ?3, model_name = ?4, num_steps = ?5,
                total_prompt_tokens = ?6, total_completion_tokens = ?7,
                start_time = ?8, end_time = ?9, atif_json = ?10, project = ?11,
                source = ?12, is_subagent = ?13, file_path = ?14, file_size = ?15,
                file_mtime_ns = ?16, collected_at_ns = ?17",
            params![
                record.session_id,
                record.schema_version,
                record.agent_name,
                record.model_name,
                record.num_steps,
                record.total_prompt_tokens,
                record.total_completion_tokens,
                record.start_time,
                record.end_time,
                record.atif_json,
                record.project,
                record.source,
                record.is_subagent as i64,
                record.file_path,
                record.file_size,
                record.file_mtime_ns,
                now_ns,
            ],
        )?;
        Ok(())
    }

    /// Fetches a stored trajectory row by session id (test/inspection helper).
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn get(&self, session_id: &str) -> Result<Option<TrajectoryRecord>> {
        let conn = self.lock_conn()?;
        let record = conn
            .query_row(
                "SELECT session_id, schema_version, agent_name, model_name, num_steps,
                        total_prompt_tokens, total_completion_tokens, start_time, end_time,
                        atif_json, project, source, is_subagent, file_path, file_size,
                        file_mtime_ns
                 FROM collected_trajectories WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok(TrajectoryRecord {
                        session_id: row.get(0)?,
                        schema_version: row.get(1)?,
                        agent_name: row.get(2)?,
                        model_name: row.get(3)?,
                        num_steps: row.get(4)?,
                        total_prompt_tokens: row.get(5)?,
                        total_completion_tokens: row.get(6)?,
                        start_time: row.get(7)?,
                        end_time: row.get(8)?,
                        atif_json: row.get(9)?,
                        project: row.get(10)?,
                        source: row.get(11)?,
                        is_subagent: row.get::<_, i64>(12)? != 0,
                        file_path: row.get(13)?,
                        file_size: row.get(14)?,
                        file_mtime_ns: row.get(15)?,
                    })
                },
            )
            .optional()?;
        Ok(record)
    }

    /// Number of stored trajectories (test/inspection helper).
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn count(&self) -> Result<i64> {
        let conn = self.lock_conn()?;
        let n = conn.query_row("SELECT COUNT(*) FROM collected_trajectories", [], |row| {
            row.get(0)
        })?;
        Ok(n)
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow!("trajectory store mutex poisoned"))
    }
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("traj-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("trajectories.db")
    }

    fn sample_record() -> TrajectoryRecord {
        TrajectoryRecord {
            session_id: "s-1".into(),
            schema_version: "ATIF-v1.7".into(),
            agent_name: "qoder".into(),
            model_name: Some("qwen-max".into()),
            num_steps: 3,
            total_prompt_tokens: Some(250),
            total_completion_tokens: Some(30),
            start_time: Some("2026-07-25T10:00:00Z".into()),
            end_time: Some("2026-07-25T10:00:05Z".into()),
            atif_json: "{\"schema_version\":\"ATIF-v1.7\"}".into(),
            project: "myapp".into(),
            source: "qoder".into(),
            is_subagent: false,
            file_path: "/root/.qoder/projects/-data-myapp/s-1.jsonl".into(),
            file_size: 1024,
            file_mtime_ns: 42,
        }
    }

    #[test]
    fn test_upsert_twice_updates_row() {
        let store = TrajectoryStore::new_with_path(&tmp_db("upsert")).unwrap();
        let mut rec = sample_record();
        store.upsert_trajectory(&rec).unwrap();

        rec.num_steps = 5;
        rec.file_size = 2048;
        rec.file_mtime_ns = 99;
        store.upsert_trajectory(&rec).unwrap();

        assert_eq!(store.count().unwrap(), 1);
        let got = store.get("s-1").unwrap().unwrap();
        assert_eq!(got.num_steps, 5);
        assert_eq!(got.file_size, 2048);
    }

    #[test]
    fn test_file_state_roundtrip() {
        let store = TrajectoryStore::new_with_path(&tmp_db("state")).unwrap();
        let rec = sample_record();
        assert!(store.get_file_state(&rec.file_path).unwrap().is_none());

        store.upsert_trajectory(&rec).unwrap();
        assert_eq!(
            store.get_file_state(&rec.file_path).unwrap(),
            Some((1024, 42))
        );
    }
}
