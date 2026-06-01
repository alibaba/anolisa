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
pub(crate) const SCHEMA_VERSION: i64 = 2;

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
                1 => Self::migrate_1_to_2(&tx)?,
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

    /// Schema v2: add `files_vec` for dense embeddings alongside FTS5.
    fn migrate_1_to_2(tx: &rusqlite::Transaction<'_>) -> Result<()> {
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS files_vec (
                path TEXT PRIMARY KEY,
                embedding BLOB NOT NULL
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
    /// Wraps everything in one transaction so `files`, `files_fts`, and
    /// `files_vec` stay consistent on partial failure (a delete that drops
    /// the FTS row but leaves the dense-vector row behind would produce an
    /// orphaned embedding that can never be matched but still wastes space
    /// and skews later vector-only search results).
    pub fn remove(&mut self, rel_path: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let prefix = format!("{rel_path}/");
        let rowids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT rowid FROM files WHERE path = ?1 OR path LIKE ?2 || '%'")?;
            let rows = stmt.query_map(params![rel_path, prefix], |r| r.get::<_, i64>(0))?;
            rows.flatten().collect()
        };
        // Same path/prefix cascade for the dense-vector table: it is keyed
        // by `path` rather than FTS rowid, so it needs its own delete pass.
        let vec_paths: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT path FROM files_vec WHERE path = ?1 OR path LIKE ?2 || '%'")?;
            let rows = stmt.query_map(params![rel_path, prefix], |r| r.get::<_, String>(0))?;
            rows.flatten().collect()
        };
        let existed = !rowids.is_empty() || !vec_paths.is_empty();
        for rid in rowids {
            tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![rid])?;
            tx.execute("DELETE FROM files WHERE rowid = ?1", params![rid])?;
        }
        for p in vec_paths {
            tx.execute("DELETE FROM files_vec WHERE path = ?1", params![p])?;
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
            let snippet: String = row.get(1)?;
            // Flag based on the snippet the caller actually sees, not the
            // full document body: a benign snippet shouldn't be marked
            // suspicious merely because the surrounding body (never shown
            // to the model) happens to contain an injection-like term.
            //
            // FTS5 wraps matched terms in '«'/»'/'…' markers; they are a
            // display artifact and would split multi-word patterns (e.g.
            // "«ignore» all «instruc»…" no longer matches the "ignore all
            // instructions" regex), so strip them before detection.
            let clean = strip_snippet_markers(&snippet);
            Ok(SearchHit {
                path: row.get::<_, String>(0)?,
                suspicious: crate::safety::looks_like_prompt_injection(&clean),
                score: row.get::<_, f64>(2)?,
                snippet,
            })
        })?;

        let out: Vec<SearchHit> = rows.flatten().collect();
        Ok(out)
    }

    /// Store a dense embedding vector for `rel_path`. The vector is
    /// serialised as a little-endian f32 BLOB.
    pub fn upsert_vec(&mut self, rel_path: &str, embedding: &[f32]) -> Result<()> {
        let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.conn.execute(
            "INSERT OR REPLACE INTO files_vec (path, embedding) VALUES (?1, ?2)",
            params![rel_path, blob],
        )?;
        Ok(())
    }

    /// Vector-only search: returns `(path, cosine_similarity)` ordered
    /// by descending similarity. The query vector is normalised and each
    /// stored vector is normalised on-the-fly so the dot product is
    /// equivalent to cosine similarity.
    pub fn search_vec(&self, query_vec: &[f32], top_k: usize) -> Result<Vec<(String, f64)>> {
        let q_norm = l2_normalise(query_vec);

        let mut stmt = self.conn.prepare("SELECT path, embedding FROM files_vec")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;

        let mut scores: Vec<(String, f64)> = Vec::new();
        for row in rows {
            let (path, blob) = match row {
                Ok(r) => r,
                Err(_) => continue,
            };
            let stored = blob_to_f32(&blob);
            if stored.len() != q_norm.len() {
                continue;
            }
            let similarity = dot_product(&q_norm, &stored) as f64;
            // Drop NaN/Inf scores: they come from malformed stored vectors
            // (e.g. zero-norm embeddings whose cosine is undefined). Keeping
            // them would make the sort comparator non-deterministic because
            // `partial_cmp(NaN, x)` is `None`, and the `unwrap_or(Equal)`
            // fallback silently shuffles such rows anywhere in the ranking.
            if !similarity.is_finite() {
                tracing::warn!("vector search: skipping non-finite similarity for {path}");
                continue;
            }
            scores.push((path, similarity));
        }

        // Sort by descending similarity. All entries are finite here, so
        // `total_cmp` is well-defined and gives a deterministic order.
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        scores.truncate(top_k);
        Ok(scores)
    }

    /// Hybrid search: combines BM25 keyword ranking with vector cosine
    /// similarity using reciprocal rank fusion (RRF, k=60).
    ///
    /// This method is the one callers should use when both the index and
    /// an embedding provider are available.
    pub fn search_hybrid(
        &self,
        query: &str,
        query_vec: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchHit>> {
        // Run both search strategies.
        let bm25_hits = self.search(query, top_k * 2);
        let vec_hits = self.search_vec(query_vec, top_k * 2);

        let (bm25_hits, vec_hits): (Vec<SearchHit>, Vec<(String, f64)>) =
            match (bm25_hits, vec_hits) {
                (Ok(b), Ok(v)) => (b, v),
                (Err(e), Ok(v)) => {
                    tracing::warn!("hybrid search: BM25 failed ({e}); falling back to vector-only");
                    (Vec::new(), v)
                }
                (Ok(b), Err(e)) => {
                    tracing::warn!("hybrid search: vector failed ({e}); falling back to BM25-only");
                    (b, Vec::new())
                }
                (Err(bm25_err), Err(vec_err)) => {
                    tracing::warn!(
                        "hybrid search: both BM25 ({bm25_err}) and vector ({vec_err}) failed"
                    );
                    return Ok(Vec::new());
                }
            };

        if bm25_hits.is_empty() && vec_hits.is_empty() {
            return Ok(Vec::new());
        }
        if vec_hits.is_empty() {
            return Ok(bm25_hits.into_iter().take(top_k).collect());
        }
        if bm25_hits.is_empty() {
            // Reconstruct SearchHit from vector-only results.
            return Ok(vec_hits
                .into_iter()
                .take(top_k)
                .map(|(path, score)| SearchHit {
                    path,
                    snippet: String::new(),
                    score,
                    suspicious: false,
                })
                .collect());
        }

        // RRF: score = Σ 1/(k + rank_i) for each result set.
        const RRF_K: f64 = 60.0;
        let mut rrf: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let mut snippets: std::collections::HashMap<String, (String, bool)> =
            std::collections::HashMap::new();

        for (rank, hit) in bm25_hits.iter().enumerate() {
            let rrf_score = 1.0 / (RRF_K + (rank as f64 + 1.0));
            *rrf.entry(hit.path.clone()).or_default() += rrf_score;
            snippets
                .entry(hit.path.clone())
                .or_insert((hit.snippet.clone(), hit.suspicious));
        }
        for (rank, (path, _)) in vec_hits.iter().enumerate() {
            let rrf_score = 1.0 / (RRF_K + (rank as f64 + 1.0));
            *rrf.entry(path.clone()).or_default() += rrf_score;
            snippets.entry(path.clone()).or_default();
        }

        let mut merged: Vec<(String, f64)> = rrf.into_iter().collect();
        // RRF scores are always finite (`1/(60+rank)`, rank≥0), so total_cmp
        // is safe and matches partial_cmp's behaviour without the NaN hole.
        merged.sort_by(|a, b| b.1.total_cmp(&a.1));
        merged.truncate(top_k);

        Ok(merged
            .into_iter()
            .map(|(path, score)| {
                let (snippet, suspicious) = snippets.remove(&path).unwrap_or_default();
                SearchHit {
                    path,
                    snippet,
                    score,
                    suspicious,
                }
            })
            .collect())
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

// ── vector helpers ─────────────────────────────────────────────

fn l2_normalise(vec: &[f32]) -> Vec<f32> {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return vec.to_vec();
    }
    vec.iter().map(|x| x / norm).collect()
}

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn blob_to_f32(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Strip FTS5 `snippet()` highlight/ellipsis markers (`«`, `»`, `…`) so
/// downstream heuristics see the underlying text. The markers are a
/// display-only artifact; leaving them in would fragment multi-word
/// patterns (e.g. "«ignore» all «instruc»…" fails the
/// "ignore all instructions" regex).
fn strip_snippet_markers(s: &str) -> String {
    // Small allocation is fine: snippets are capped at ~16 FTS tokens.
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '«' | '»' | '…' => {}
            other => out.push(other),
        }
    }
    out
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
    fn remove_drops_orphaned_files_vec_rows() {
        // Regression: pre-fix `remove()` deleted the FTS row but left the
        // dense-vector row in `files_vec` behind. An orphaned vector can
        // never be matched by a BM25 query but still resurfaces in
        // vector/hybrid search and skews ranking, so removal must purge
        // both tables in the same transaction.
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert("notes/a.md", 0, 0, "alpha").unwrap();
        s.upsert_vec("notes/a.md", &[0.1, 0.2, 0.3]).unwrap();
        // Sanity: vector search finds the row before removal.
        let pre = s.search_vec(&[0.1, 0.2, 0.3], 5).unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].0, "notes/a.md");

        assert!(s.remove("notes/a.md").unwrap());

        // FTS row gone.
        assert!(s.search("alpha", 5).unwrap().is_empty());
        // Vector row also gone — no orphan left behind.
        let post = s.search_vec(&[0.1, 0.2, 0.3], 5).unwrap();
        assert!(post.is_empty(), "orphaned vector row survived remove");
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
