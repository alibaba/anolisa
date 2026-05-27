use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use rusqlite::{Connection, params};

use crate::error::{MemoryError, Result};

use super::SearchHit;

/// SQLite FTS5 BM25 backend used by IndexWorker. All access goes through
/// the inner Connection — guarded by an external Mutex in IndexHandle,
/// which is why mutating methods take `&mut self` (the MutexGuard
/// already provides exclusive access; we use it to drive `transaction`).
pub struct BM25Store {
    conn: Connection,
}

/// Latest schema version this binary knows how to produce.
/// On open, an older DB is upgraded step-by-step until it reaches this
/// version; a newer DB causes the open to fail so a downgraded binary
/// doesn't silently corrupt rows it doesn't understand.
pub(crate) const SCHEMA_VERSION: i64 = 1;

impl BM25Store {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        // Modest sensible defaults: WAL gives concurrent readers while a
        // writer is committing (today everything is serialised through
        // IndexHandle's Mutex but it costs nothing); busy_timeout shields
        // against external SQLite tools probing the file. NORMAL synchronous
        // is the WAL-recommended setting (full fsync per checkpoint, not
        // per commit).
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

        Self::ensure_schema(&mut conn)?;
        Ok(Self { conn })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        Self::ensure_schema(&mut conn)?;
        Ok(Self { conn })
    }

    /// Ensure the open connection's schema is at SCHEMA_VERSION.
    /// - Fresh DB (version 0) → apply the v1 baseline.
    /// - Older DB → step through `migrate_<N>_to_<N+1>` until current.
    /// - Newer DB → fail loudly (refuse to operate on unknown schema).
    fn ensure_schema(conn: &mut Connection) -> Result<()> {
        let current: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);

        if current > SCHEMA_VERSION {
            return Err(MemoryError::Other(format!(
                "index db schema is at v{current}, binary only supports up to v{SCHEMA_VERSION}; \
                 downgrade is not safe"
            )));
        }

        if current == SCHEMA_VERSION {
            return Ok(());
        }

        // Each migration runs inside its own transaction so a crash mid-
        // upgrade either leaves the DB at the previous version or the next.
        let mut at = current;
        while at < SCHEMA_VERSION {
            let tx = conn.transaction()?;
            match at {
                0 => Self::migrate_0_to_1(&tx)?,
                // Future steps insert here, each bumping `at`.
                n => {
                    return Err(MemoryError::Other(format!(
                        "no migration registered from schema v{n} to v{}",
                        n + 1
                    )));
                }
            }
            at += 1;
            tx.pragma_update(None, "user_version", at)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Initial schema (v1): file metadata table + FTS5 BM25 over body.
    fn migrate_0_to_1(tx: &rusqlite::Transaction<'_>) -> Result<()> {
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS files (
                rowid       INTEGER PRIMARY KEY,
                path        TEXT NOT NULL UNIQUE,
                mtime_ms    INTEGER NOT NULL,
                size        INTEGER NOT NULL,
                indexed_at  TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
                path UNINDEXED,
                body,
                tokenize='trigram'
            );
            "#,
        )?;
        Ok(())
    }

    /// Insert or replace a file's index entry. `body` is the extracted
    /// text. All writes happen inside one transaction so a crash mid-
    /// upsert can't leave `files` and `files_fts` out of sync.
    pub fn upsert(&mut self, rel_path: &str, mtime_ms: i64, size: u64, body: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let tx = self.conn.transaction()?;
        let existing_rowid: Option<i64> = tx
            .query_row(
                "SELECT rowid FROM files WHERE path = ?1",
                params![rel_path],
                |r| r.get(0),
            )
            .ok();

        match existing_rowid {
            Some(rowid) => {
                tx.execute(
                    "UPDATE files SET mtime_ms=?1, size=?2, indexed_at=?3 WHERE rowid=?4",
                    params![mtime_ms, size as i64, now, rowid],
                )?;
                tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![rowid])?;
                tx.execute(
                    "INSERT INTO files_fts(rowid, path, body) VALUES (?1, ?2, ?3)",
                    params![rowid, rel_path, body],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO files (path, mtime_ms, size, indexed_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![rel_path, mtime_ms, size as i64, now],
                )?;
                let rowid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO files_fts(rowid, path, body) VALUES (?1, ?2, ?3)",
                    params![rowid, rel_path, body],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Remove a file's index entry. Returns true if any row existed.
    ///
    /// Cascade semantics: if `rel_path` matches a stored row exactly, that
    /// row is removed. Additionally, any descendant whose path starts with
    /// `rel_path + "/"` is removed too — this matters when a *directory* is
    /// renamed or moved out of the tree, in which case notify may not emit
    /// per-file unlinks for every leaf. Without the cascade those rows
    /// would linger as stale FTS hits forever.
    ///
    /// Wraps everything in one transaction so `files` and `files_fts` stay
    /// consistent on partial failure.
    pub fn remove(&mut self, rel_path: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let prefix = format!("{rel_path}/");
        let rowids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT rowid FROM files WHERE path = ?1 OR path LIKE ?2 || '%'")?;
            let rows = stmt.query_map(params![rel_path, prefix], |r| r.get::<_, i64>(0))?;
            rows.flatten().collect()
        };
        let existed = !rowids.is_empty();
        for rid in rowids {
            tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![rid])?;
            tx.execute("DELETE FROM files WHERE rowid = ?1", params![rid])?;
        }
        tx.commit()?;
        Ok(existed)
    }

    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() {
            return Err(MemoryError::InvalidArgument("empty search query".into()));
        }
        let fts_q = sanitize_fts_query(query);
        if fts_q.is_empty() {
            return Ok(Vec::new());
        }

        let sql = r#"
            SELECT path,
                   snippet(files_fts, 1, '«', '»', '…', 16) AS snip,
                   bm25(files_fts) AS rank
            FROM files_fts
            WHERE files_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
        "#;
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![fts_q, top_k as i64], |row| {
            Ok(SearchHit {
                path: row.get::<_, String>(0)?,
                snippet: row.get::<_, String>(1)?,
                score: row.get::<_, f64>(2)?,
            })
        })?;

        let out: Vec<SearchHit> = rows.flatten().collect();
        Ok(out)
    }

    pub fn count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    pub fn known_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT path FROM files")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let out: Vec<String> = rows.flatten().collect();
        Ok(out)
    }

    pub fn mtime_for(&self, rel_path: &str) -> Option<i64> {
        self.conn
            .query_row(
                "SELECT mtime_ms FROM files WHERE path = ?1",
                params![rel_path],
                |r| r.get(0),
            )
            .ok()
    }
}

/// Convert a raw query into something safe for FTS5: drop quotes /
/// punctuation that confuse the parser, AND-join surviving tokens.
/// `-` is dropped because FTS5 interprets a leading `-` as the NOT
/// operator, so naïvely keeping it would silently invert match intent
/// (`hello-world` → match docs containing "hello" but NOT "world").
fn sanitize_fts_query(q: &str) -> String {
    q.split_whitespace()
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '.'))
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn mtime_ms_of(meta: &std::fs::Metadata) -> i64 {
    let dur = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
    match dur {
        Some(d) => d.as_millis() as i64,
        None => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_search_remove_roundtrip() {
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert("notes/a.md", 100, 10, "rust loves ownership")
            .unwrap();
        s.upsert("notes/b.md", 100, 10, "python uses gc").unwrap();

        let hits = s.search("rust", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "notes/a.md");

        s.remove("notes/a.md").unwrap();
        let hits = s.search("rust", 5).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn search_handles_chinese() {
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert("a.md", 0, 0, "你好世界 hello").unwrap();
        let hits = s.search("hello", 5).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn empty_query_errors() {
        let s = BM25Store::open_in_memory().unwrap();
        assert!(matches!(
            s.search("   ", 5),
            Err(MemoryError::InvalidArgument(_))
        ));
    }

    #[test]
    fn remove_cascades_to_dir_children() {
        // Regression: pre-fix `remove("notes")` only deleted a row with
        // exact path "notes" and left `notes/a.md` + `notes/sub/b.md`
        // behind as stale FTS hits. With the cascade, removing the dir
        // prefix nukes every descendant in one transaction.
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert("notes/a.md", 0, 0, "alpha").unwrap();
        s.upsert("notes/sub/b.md", 0, 0, "beta").unwrap();
        s.upsert("other/c.md", 0, 0, "gamma").unwrap();

        let existed = s.remove("notes").unwrap();
        assert!(existed, "removing a populated prefix must report true");

        let paths = s.known_paths().unwrap();
        assert_eq!(paths, vec!["other/c.md".to_string()]);
        // FTS row for the cascaded body is also gone.
        let hits = s.search("alpha", 5).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn ensure_schema_is_idempotent() {
        // Re-opening an existing on-disk DB must be a no-op once schema
        // is at SCHEMA_VERSION; ensure_schema reads user_version and
        // returns early instead of re-running migrations.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        {
            let mut s = BM25Store::open(path).unwrap();
            s.upsert("a.md", 1, 1, "x").unwrap();
        }
        // Second open must succeed and preserve data.
        let s = BM25Store::open(path).unwrap();
        assert_eq!(s.count().unwrap(), 1);
    }

    #[test]
    fn ensure_schema_rejects_newer_db() {
        // Simulate a DB written by a future binary (user_version > SCHEMA_VERSION).
        // ensure_schema must refuse to operate rather than risk corrupting
        // rows it doesn't understand.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        {
            let conn = Connection::open(path).unwrap();
            conn.execute_batch("PRAGMA user_version = 999;").unwrap();
        }
        // BM25Store doesn't impl Debug (Connection isn't Debug), so we
        // collect the error message by hand for the assertion.
        let err_msg = match BM25Store::open(path) {
            Ok(_) => "Ok(BM25Store)".to_string(),
            Err(e) => format!("Err({e})"),
        };
        assert!(
            err_msg.contains("downgrade"),
            "expected downgrade-refusal error, got: {err_msg}"
        );
    }

    #[test]
    fn upsert_replaces_fts_row_atomically() {
        // Regression: pre-fix the files / files_fts updates ran outside
        // a transaction. A crash between the two left files with the
        // new mtime but no FTS row (or vice versa). With the transaction
        // wrap, a successful upsert always has both, and a successful
        // remove always has neither.
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert("doc.md", 1, 5, "alpha").unwrap();
        // Re-upsert with new body; FTS row should match the new body.
        s.upsert("doc.md", 2, 5, "omega").unwrap();
        let hits = s.search("omega", 5).unwrap();
        assert_eq!(hits.len(), 1);
        let hits = s.search("alpha", 5).unwrap();
        assert!(hits.is_empty(), "old FTS body should be gone");
    }
}
