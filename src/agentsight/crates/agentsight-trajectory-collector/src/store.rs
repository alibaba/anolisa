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
use serde::Serialize;

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
    /// First user-authored message preview (≤ 200 chars), from ATIF steps.
    pub first_user_message: Option<String>,
    /// Last user-authored message preview (≤ 200 chars), from ATIF steps.
    pub last_user_message: Option<String>,
    pub atif_json: String,
    // Collection bookkeeping columns
    pub project: String,
    pub source: String,
    pub is_subagent: bool,
    pub file_path: String,
    pub file_size: i64,
    pub file_mtime_ns: i64,
}

/// Lightweight list row of `collected_trajectories` — deliberately excludes
/// the (potentially large) `atif_json` column; fetch it via
/// [`TrajectoryStore::get_atif_json`] for the detail view.
#[derive(Debug, Clone, Serialize)]
pub struct TrajectorySummary {
    pub session_id: String,
    pub schema_version: String,
    pub agent_name: String,
    pub model_name: Option<String>,
    pub num_steps: i64,
    pub total_prompt_tokens: Option<i64>,
    pub total_completion_tokens: Option<i64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub first_user_message: Option<String>,
    pub last_user_message: Option<String>,
    pub project: String,
    /// Which product wrote the file: "qoder" or "qoderwork".
    pub source: String,
    pub is_subagent: bool,
    pub collected_at_ns: i64,
}

/// Distinct filter values for the trajectory list UI dropdowns.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TrajectoryFilters {
    pub projects: Vec<String>,
    pub sources: Vec<String>,
    pub agent_names: Vec<String>,
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
                collected_at_ns INTEGER NOT NULL,
                first_user_message TEXT,
                last_user_message TEXT
            )",
            [],
        )?;
        // Lightweight bookkeeping for files that failed conversion (corrupted
        // JSONL, empty events, etc.) so they are not re-read every scan round.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS skipped_files (
                file_path TEXT PRIMARY KEY,
                file_size INTEGER NOT NULL,
                file_mtime_ns INTEGER NOT NULL
            )",
            [],
        )?;
        migrate_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Returns `(file_size, file_mtime_ns)` recorded for `file_path`, if any.
    /// Drives the incremental scan: unchanged files are skipped.
    /// Checks both successfully ingested trajectories and skipped (corrupted)
    /// files.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn get_file_state(&self, file_path: &str) -> Result<Option<(i64, i64)>> {
        let conn = self.lock_conn()?;
        // First check successfully ingested files.
        let state = conn
            .query_row(
                "SELECT file_size, file_mtime_ns FROM collected_trajectories
                 WHERE file_path = ?1",
                params![file_path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if state.is_some() {
            return Ok(state);
        }
        // Then check skipped (corrupted) files.
        let skipped = conn
            .query_row(
                "SELECT file_size, file_mtime_ns FROM skipped_files
                 WHERE file_path = ?1",
                params![file_path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(skipped)
    }

    /// Records the file state for a file that failed conversion, preventing
    /// re-reads until the file changes (size/mtime differ).
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn set_file_state(
        &self,
        file_path: &str,
        file_size: i64,
        file_mtime_ns: i64,
    ) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO skipped_files (file_path, file_size, file_mtime_ns)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(file_path) DO UPDATE SET file_size = ?2, file_mtime_ns = ?3",
            params![file_path, file_size, file_mtime_ns],
        )?;
        Ok(())
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
                file_mtime_ns, collected_at_ns, first_user_message, last_user_message
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(session_id) DO UPDATE SET
                schema_version = ?2, agent_name = ?3, model_name = ?4, num_steps = ?5,
                total_prompt_tokens = ?6, total_completion_tokens = ?7,
                start_time = ?8, end_time = ?9, atif_json = ?10, project = ?11,
                source = ?12, is_subagent = ?13, file_path = ?14, file_size = ?15,
                file_mtime_ns = ?16, collected_at_ns = ?17,
                first_user_message = ?18, last_user_message = ?19",
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
                record.first_user_message,
                record.last_user_message,
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
                        file_mtime_ns, first_user_message, last_user_message
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
                        first_user_message: row.get(16)?,
                        last_user_message: row.get(17)?,
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

    /// Lists trajectory summaries (without `atif_json`), newest first.
    ///
    /// All filters are optional equality matches; `limit` caps the row count.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn list_summaries(
        &self,
        project: Option<&str>,
        source: Option<&str>,
        agent_name: Option<&str>,
        limit: i64,
    ) -> Result<Vec<TrajectorySummary>> {
        let conn = self.lock_conn()?;
        let mut sql = String::from(
            "SELECT session_id, schema_version, agent_name, model_name, num_steps,
                    total_prompt_tokens, total_completion_tokens, start_time, end_time,
                    project, source, is_subagent, collected_at_ns,
                    first_user_message, last_user_message
             FROM collected_trajectories",
        );
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(p) = project {
            clauses.push("project = ?".to_string());
            args.push(Box::new(p.to_string()));
        }
        if let Some(s) = source {
            clauses.push("source = ?".to_string());
            args.push(Box::new(s.to_string()));
        }
        if let Some(a) = agent_name {
            clauses.push("agent_name = ?".to_string());
            args.push(Box::new(a.to_string()));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        // `limit` is bound as a parameter (never interpolated) to stay injection-free.
        sql.push_str(&format!(
            " ORDER BY collected_at_ns DESC LIMIT ?{}",
            args.len() + 1
        ));
        args.push(Box::new(limit));

        let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(TrajectorySummary {
                session_id: row.get(0)?,
                schema_version: row.get(1)?,
                agent_name: row.get(2)?,
                model_name: row.get(3)?,
                num_steps: row.get(4)?,
                total_prompt_tokens: row.get(5)?,
                total_completion_tokens: row.get(6)?,
                start_time: row.get(7)?,
                end_time: row.get(8)?,
                project: row.get(9)?,
                source: row.get(10)?,
                is_subagent: row.get::<_, i64>(11)? != 0,
                collected_at_ns: row.get(12)?,
                first_user_message: row.get(13)?,
                last_user_message: row.get(14)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Returns the stored ATIF v1.7 JSON for one session, if present.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn get_atif_json(&self, session_id: &str) -> Result<Option<String>> {
        let conn = self.lock_conn()?;
        let json = conn
            .query_row(
                "SELECT atif_json FROM collected_trajectories WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json)
    }

    /// Returns the ATIF JSON strings of all subagent trajectories belonging to
    /// the given parent session (matching `<parent>:subagent:%`).
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn get_subagent_atif_jsons(&self, parent_session_id: &str) -> Result<Vec<String>> {
        let conn = self.lock_conn()?;
        let pattern = format!("{parent_session_id}:subagent:%");
        let mut stmt =
            conn.prepare("SELECT atif_json FROM collected_trajectories WHERE session_id LIKE ?1")?;
        let rows = stmt.query_map(params![pattern], |row| row.get(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Returns distinct project / source / agent_name values for UI filters.
    ///
    /// # Errors
    /// Returns an error on SQL failure or poisoned mutex.
    pub fn list_filters(&self) -> Result<TrajectoryFilters> {
        let conn = self.lock_conn()?;
        Ok(TrajectoryFilters {
            projects: distinct_column(&conn, "project")?,
            sources: distinct_column(&conn, "source")?,
            agent_names: distinct_column(&conn, "agent_name")?,
        })
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

/// Current schema version recorded in `PRAGMA user_version`.
/// v1: preview columns added; v2: previews recomputed with system-context
/// stripping (see [`extract_user_message_previews`]).
const SCHEMA_USER_VERSION: i32 = 2;

/// Max characters kept per user-message preview column.
const MESSAGE_PREVIEW_CHARS: usize = 200;

/// XML-style tag pairs injected into user turns by IDEs/CLIs (Qoder slash
/// commands, Claude Code local-command transcripts, system reminders). Their
/// content is not genuine user text and must not surface in previews.
const SYSTEM_TAG_PAIRS: &[(&str, &str)] = &[
    ("<system-reminder>", "</system-reminder>"),
    ("<current_notes_content>", "</current_notes_content>"),
    ("<command-message>", "</command-message>"),
    ("<command-name>", "</command-name>"),
    ("<command-args>", "</command-args>"),
    ("<local-command-caveat>", "</local-command-caveat>"),
    ("<local-command-stdout>", "</local-command-stdout>"),
    ("<local-command-stderr>", "</local-command-stderr>"),
];

/// Remove system-injected tag blocks from a user message. An unterminated
/// opening tag swallows the rest of the text (matches AgentOpt semantics).
fn strip_system_context(text: &str) -> String {
    let mut result = text.to_string();
    for &(open, close) in SYSTEM_TAG_PAIRS {
        while let Some(start) = result.find(open) {
            if let Some(end_rel) = result[start..].find(close) {
                let end = start + end_rel + close.len();
                result.replace_range(start..end, "");
            } else {
                result.truncate(start);
                break;
            }
        }
    }
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result.trim().to_string()
}

/// One-shot schema migration for databases created before the user-message
/// preview columns existed: adds the columns (no-op on fresh databases where
/// `CREATE TABLE` already includes them) and (re)computes them from the
/// stored ATIF JSON. v1→v2 recomputes every row because v2 added
/// system-context stripping to the extractor.
fn migrate_schema(conn: &Connection) -> Result<()> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= SCHEMA_USER_VERSION {
        return Ok(());
    }
    for col in ["first_user_message", "last_user_message"] {
        let sql = format!("ALTER TABLE collected_trajectories ADD COLUMN {col} TEXT");
        if let Err(e) = conn.execute(&sql, []) {
            // Fresh databases already have the column via CREATE TABLE.
            if !e.to_string().contains("duplicate column name") {
                return Err(e.into());
            }
        }
    }
    backfill_message_previews(conn)?;
    conn.pragma_update(None, "user_version", SCHEMA_USER_VERSION)?;
    Ok(())
}

/// Recomputes the preview columns for every row from its `atif_json`.
fn backfill_message_previews(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT session_id, atif_json FROM collected_trajectories")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    for (session_id, atif_json) in rows {
        let (first, last) = extract_user_message_previews(&atif_json);
        conn.execute(
            "UPDATE collected_trajectories
             SET first_user_message = ?2, last_user_message = ?3
             WHERE session_id = ?1",
            params![session_id, first, last],
        )?;
    }
    Ok(())
}

/// Extracts first/last user-authored message previews (≤ 200 chars) from an
/// ATIF JSON document. System-injected tag blocks are stripped first; user
/// steps left empty after stripping are skipped. Parses generically
/// (tolerant of schema drift) and returns `(None, None)` on malformed input
/// or when no user step carries genuine text.
pub fn extract_user_message_previews(atif_json: &str) -> (Option<String>, Option<String>) {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(atif_json) else {
        return (None, None);
    };
    let Some(steps) = doc.get("steps").and_then(|s| s.as_array()) else {
        return (None, None);
    };
    let mut first = None;
    let mut last = None;
    for step in steps {
        if step.get("source").and_then(|s| s.as_str()) != Some("user") {
            continue;
        }
        let Some(msg) = step.get("message").and_then(|m| m.as_str()) else {
            continue;
        };
        let cleaned = strip_system_context(msg);
        if cleaned.is_empty() {
            continue;
        }
        let preview: String = cleaned.chars().take(MESSAGE_PREVIEW_CHARS).collect();
        if first.is_none() {
            first = Some(preview.clone());
        }
        last = Some(preview);
    }
    (first, last)
}

/// SELECT DISTINCT on a fixed column name (hard-coded, never user input).
fn distinct_column(conn: &Connection, column: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT {column} FROM collected_trajectories ORDER BY {column}"
    ))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
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
            first_user_message: Some("修复登录 bug".into()),
            last_user_message: Some("再跑一遍测试".into()),
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

    #[test]
    fn test_list_summaries_filters_and_limit() {
        let store = TrajectoryStore::new_with_path(&tmp_db("list")).unwrap();
        let mut a = sample_record();
        a.session_id = "a".into();
        a.project = "p1".into();
        a.source = "qoder".into();
        a.agent_name = "qoder".into();
        store.upsert_trajectory(&a).unwrap();

        let mut b = sample_record();
        b.session_id = "b".into();
        b.project = "p2".into();
        b.source = "qoderwork".into();
        b.agent_name = "qoder".into();
        b.file_path = "/root/.qoderwork/projects/x/b.jsonl".into();
        store.upsert_trajectory(&b).unwrap();

        // No filter → both rows.
        assert_eq!(
            store.list_summaries(None, None, None, 100).unwrap().len(),
            2
        );
        // project filter
        let p1 = store.list_summaries(Some("p1"), None, None, 100).unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].session_id, "a");
        // source filter
        let qw = store
            .list_summaries(None, Some("qoderwork"), None, 100)
            .unwrap();
        assert_eq!(qw.len(), 1);
        assert_eq!(qw[0].session_id, "b");
        // combined filters (AND)
        assert_eq!(
            store
                .list_summaries(Some("p1"), Some("qoderwork"), None, 100)
                .unwrap()
                .len(),
            0
        );
        // limit caps the result
        assert_eq!(store.list_summaries(None, None, None, 1).unwrap().len(), 1);
    }

    #[test]
    fn test_get_atif_json_hit_and_miss() {
        let store = TrajectoryStore::new_with_path(&tmp_db("atif")).unwrap();
        store.upsert_trajectory(&sample_record()).unwrap();
        assert_eq!(
            store.get_atif_json("s-1").unwrap().as_deref(),
            Some("{\"schema_version\":\"ATIF-v1.7\"}")
        );
        assert!(store.get_atif_json("nope").unwrap().is_none());
    }

    #[test]
    fn test_extract_user_message_previews() {
        let atif = r#"{"steps":[
            {"step_id":1,"source":"system","message":"sys prompt"},
            {"step_id":2,"source":"user","message":"  第一条用户消息  "},
            {"step_id":3,"source":"agent","message":"好的"},
            {"step_id":4,"source":"user","message":"最后一条"}
        ]}"#;
        let (first, last) = extract_user_message_previews(atif);
        assert_eq!(first.as_deref(), Some("第一条用户消息"));
        assert_eq!(last.as_deref(), Some("最后一条"));

        // Malformed / empty inputs degrade to (None, None).
        assert_eq!(extract_user_message_previews("not json"), (None, None));
        assert_eq!(extract_user_message_previews("{}"), (None, None));
        let no_user = r#"{"steps":[{"step_id":1,"source":"agent","message":"x"}]}"#;
        assert_eq!(extract_user_message_previews(no_user), (None, None));

        // Long messages are truncated to 200 chars.
        let long_msg = "啦".repeat(300);
        let atif_long =
            format!(r#"{{"steps":[{{"step_id":1,"source":"user","message":"{long_msg}"}}]}}"#);
        let (first, _) = extract_user_message_previews(&atif_long);
        assert_eq!(first.map(|s| s.chars().count()), Some(200));
    }

    #[test]
    fn test_extract_strips_system_injected_tags() {
        // Slash-command turns and local-command caveats are not user text;
        // the preview must fall through to the first genuine message.
        let atif = r#"{"steps":[
            {"step_id":1,"source":"user","message":"<command-message>clear</command-message> <command-name>/clear</command-name>"},
            {"step_id":2,"source":"user","message":"<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>\n那你看看当前能做到么"},
            {"step_id":3,"source":"user","message":"<system-reminder>injected memo</system-reminder>再跑一遍测试"}
        ]}"#;
        let (first, last) = extract_user_message_previews(atif);
        assert_eq!(first.as_deref(), Some("那你看看当前能做到么"));
        assert_eq!(last.as_deref(), Some("再跑一遍测试"));

        // Unterminated tag swallows the rest of the block.
        let atif2 = r#"{"steps":[{"step_id":1,"source":"user","message":"<local-command-caveat>Caveat: truncated"}]}"#;
        assert_eq!(extract_user_message_previews(atif2), (None, None));
    }

    #[test]
    fn test_migration_backfills_legacy_rows() {
        let db = tmp_db("migrate");
        // Seed a row with stale previews (as written by schema v1, without
        // system-context stripping), then reset user_version to simulate a
        // legacy database.
        {
            let store = TrajectoryStore::new_with_path(&db).unwrap();
            let mut rec = sample_record();
            rec.first_user_message = Some("<command-message>clear</command-message>".into());
            rec.last_user_message = None;
            rec.atif_json = r#"{"steps":[
                {"step_id":1,"source":"user","message":"<command-message>clear</command-message>"},
                {"step_id":2,"source":"user","message":"首条"},
                {"step_id":3,"source":"user","message":"末条"}
            ]}"#
            .into();
            store.upsert_trajectory(&rec).unwrap();
        }
        {
            let conn = Connection::open(&db).unwrap();
            conn.pragma_update(None, "user_version", 0).unwrap();
        }

        // Reopen → migration recomputes previews from atif_json, replacing
        // the stale tag-polluted value.
        let store = TrajectoryStore::new_with_path(&db).unwrap();
        let got = store.get("s-1").unwrap().unwrap();
        assert_eq!(got.first_user_message.as_deref(), Some("首条"));
        assert_eq!(got.last_user_message.as_deref(), Some("末条"));

        let rows = store.list_summaries(None, None, None, 10).unwrap();
        assert_eq!(rows[0].first_user_message.as_deref(), Some("首条"));
        assert_eq!(rows[0].last_user_message.as_deref(), Some("末条"));
    }

    #[test]
    fn test_list_filters_distinct() {
        let store = TrajectoryStore::new_with_path(&tmp_db("filters")).unwrap();
        let mut a = sample_record();
        a.session_id = "a".into();
        a.project = "p1".into();
        a.source = "qoder".into();
        a.agent_name = "qoder".into();
        store.upsert_trajectory(&a).unwrap();

        let mut b = sample_record();
        b.session_id = "b".into();
        b.project = "p1".into();
        b.source = "qoderwork".into();
        b.agent_name = "qoder".into();
        b.file_path = "/root/.qoderwork/projects/x/b.jsonl".into();
        store.upsert_trajectory(&b).unwrap();

        let f = store.list_filters().unwrap();
        assert_eq!(f.projects, vec!["p1".to_string()]);
        assert_eq!(
            f.sources,
            vec!["qoder".to_string(), "qoderwork".to_string()]
        );
        assert_eq!(f.agent_names, vec!["qoder".to_string()]);
    }
}
