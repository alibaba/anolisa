use std::path::{Path, PathBuf};
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
    /// Time decay lambda for recency-based ranking. `exp(-lambda * age_days)`.
    /// When 0.0, time decay is disabled (default behavior).
    time_decay_lambda: f64,
    /// Time decay alpha: weight of time factor added to search scores.
    time_decay_alpha: f64,
    /// Whether normal search excludes cold files.
    exclude_cold_on_search: bool,
    /// Mount root — derived from the db path so `supersede()` can safely
    /// update on-disk frontmatter without trusting environment variables.
    mount_root: PathBuf,
}

/// Latest schema version this binary knows how to produce.
/// On open, an older DB is upgraded step-by-step until it reaches this
/// version; a newer DB causes the open to fail so a downgraded binary
/// doesn't silently corrupt rows it doesn't understand.
pub(crate) const SCHEMA_VERSION: i64 = 5;

impl BM25Store {
    pub fn open(
        path: &Path,
        time_decay_lambda: f64,
        time_decay_alpha: f64,
        exclude_cold_on_search: bool,
    ) -> Result<Self> {
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

        // Derive mount_root from db_path: bm25.db lives at
        // <mount_root>/.anolisa/index/bm25.db, so mount_root is three
        // parents up. Fall back to cwd — the disk frontmatter update
        // is best-effort.
        let mount_root = path
            .parent() // index/
            .and_then(|p| p.parent()) // .anolisa/
            .and_then(|p| p.parent()) // <mount_root>
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        Self::ensure_schema(&mut conn)?;
        Ok(Self {
            conn,
            time_decay_lambda,
            time_decay_alpha,
            exclude_cold_on_search,
            mount_root,
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with(0.01, 0.3, true)
    }

    #[cfg(test)]
    pub fn open_in_memory_with(
        time_decay_lambda: f64,
        time_decay_alpha: f64,
        exclude_cold: bool,
    ) -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        Self::ensure_schema(&mut conn)?;
        Ok(Self {
            conn,
            time_decay_lambda,
            time_decay_alpha,
            exclude_cold_on_search: exclude_cold,
            mount_root: PathBuf::new(),
        })
    }

    #[cfg(test)]
    fn open_for_test(path: &Path) -> Result<Self> {
        Self::open(path, 0.01, 0.3, true)
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
                2 => Self::migrate_2_to_3(&tx)?,
                3 => Self::migrate_3_to_4(&tx)?,
                4 => Self::migrate_4_to_5(&tx)?,
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

    /// Schema v3: add cold tracking columns to `files`.
    fn migrate_2_to_3(tx: &rusqlite::Transaction<'_>) -> Result<()> {
        tx.execute_batch(
            r#"
            ALTER TABLE files ADD COLUMN access_count INTEGER DEFAULT 0;
            ALTER TABLE files ADD COLUMN last_accessed_ms INTEGER DEFAULT 0;
            ALTER TABLE files ADD COLUMN is_cold INTEGER DEFAULT 0;
            "#,
        )?;
        Ok(())
    }

    /// Schema v4: add `is_superseded` for conflict resolution.
    fn migrate_3_to_4(tx: &rusqlite::Transaction<'_>) -> Result<()> {
        tx.execute(
            "ALTER TABLE files ADD COLUMN is_superseded INTEGER DEFAULT 0",
            [],
        )?;
        Ok(())
    }

    /// Schema v5: add agent_id column for per-agent memory scoping.
    fn migrate_4_to_5(tx: &rusqlite::Transaction<'_>) -> Result<()> {
        tx.execute(
            "ALTER TABLE files ADD COLUMN agent_id TEXT DEFAULT NULL",
            [],
        )?;
        Ok(())
    }

    /// Insert or replace a file's index entry. `body` is the extracted
    /// text. All writes happen inside one transaction so a crash mid-
    /// upsert can't leave `files` and `files_fts` out of sync.
    pub fn upsert(
        &mut self,
        rel_path: &str,
        mtime_ms: i64,
        size: u64,
        body: &str,
        agent_id: Option<&str>,
    ) -> Result<()> {
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
                    "UPDATE files SET mtime_ms=?1, size=?2, indexed_at=?3, agent_id=COALESCE(agent_id, ?4) WHERE rowid=?5",
                    params![mtime_ms, size as i64, now, agent_id, rowid],
                )?;
                tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![rowid])?;
                tx.execute(
                    "INSERT INTO files_fts(rowid, path, body) VALUES (?1, ?2, ?3)",
                    params![rowid, rel_path, body],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO files (path, mtime_ms, size, indexed_at, agent_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![rel_path, mtime_ms, size as i64, now, agent_id],
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
        // Cascade: remove corresponding vector embeddings.
        tx.execute(
            "DELETE FROM files_vec WHERE path = ?1 OR path LIKE ?2 || '%'",
            params![rel_path, prefix],
        )?;
        tx.commit()?;
        Ok(existed)
    }

    pub fn search(&self, query: &str, top_k: usize, exclude_cold: bool) -> Result<Vec<SearchHit>> {
        self.search_scoped(query, top_k, exclude_cold, None)
    }

    /// Search with optional agent scope filter.
    /// `agent_scope` can be:
    /// - None: return all results (shared mode, default)
    /// - Some("isolated:<agent_id>"): only results tagged with this agent_id
    /// - Some("filter:<agent_id>"): results tagged with this agent_id plus
    ///   any unscoped (agent_id IS NULL) memories
    ///
    /// Returns `InvalidArgument` when the scope prefix is recognised but the
    /// agent_id contains characters that would let it escape the parameterised
    /// binding path (`'`, `"`, `;`, `\`, `/`, control bytes). Callers must
    /// surface the error rather than silently falling back to shared mode,
    /// otherwise a misconfigured `MCP_CLIENT_NAME` would silently widen the
    /// visibility domain.
    pub fn search_scoped(
        &self,
        query: &str,
        top_k: usize,
        exclude_cold: bool,
        agent_scope: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() {
            return Err(MemoryError::InvalidArgument("empty search query".into()));
        }
        let fts_q = sanitize_fts_query(query);
        if fts_q.is_empty() {
            return Ok(Vec::new());
        }

        let cold_filter = if exclude_cold {
            "AND f.is_cold = 0"
        } else {
            ""
        };
        let superseded_filter = "AND f.is_superseded = 0";

        // Agent scope filter: when set, only return results from the specified agent.
        // Use parameterised binding (`?3`) rather than `format!()` so an
        // attacker-controlled `MCP_CLIENT_NAME` cannot escape the literal.
        // `/` and `\` are rejected so a client name like `org/team/agent`
        // cannot be confused with a path component elsewhere in the query.
        let (agent_filter, agent_param) = match agent_scope {
            Some(scope) if scope.starts_with("isolated:") || scope.starts_with("filter:") => {
                let agent_id = scope.split_once(':').map(|x| x.1).unwrap_or("");
                if agent_id.contains(|c: char| {
                    c == '\''
                        || c == '"'
                        || c == ';'
                        || c == '\\'
                        || c == '/'
                        || c == '\0'
                        || c == '\n'
                        || c == '\r'
                }) {
                    return Err(MemoryError::InvalidArgument(format!(
                        "agent_scope contains invalid characters: {agent_id:?}"
                    )));
                }
                // isolated: only the agent's own memories.
                // filter: agent's own plus unscoped (NULL) memories.
                let sql_filter = if scope.starts_with("isolated:") {
                    "AND f.agent_id = ?3"
                } else {
                    "AND (f.agent_id = ?3 OR f.agent_id IS NULL)"
                };
                (sql_filter.to_string(), Some(agent_id.to_string()))
            }
            // Unknown prefix or shared -> no filter (parameter list stays at 2).
            _ => (String::new(), None),
        };

        // Join with files to get mtime for time decay.
        let sql = format!(
            r#"
            SELECT f.path,
                   snippet(files_fts, 1, '«', '»', '…', 16) AS snip,
                   bm25(files_fts) AS rank,
                   body,
                   f.mtime_ms
            FROM files_fts
            JOIN files f ON f.rowid = files_fts.rowid
            WHERE files_fts MATCH ?1 {cold_filter} {superseded_filter} {agent_filter}
            ORDER BY rank
            LIMIT ?2
        "#
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<(String, String, f64, String, i64)> = if let Some(ref agent_id) = agent_param
        {
            stmt.query_map(params![fts_q, top_k as i64, agent_id], |row| {
                let body: String = row.get(3)?;
                let mtime_ms: i64 = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    body,
                    mtime_ms,
                ))
            })?
            .flatten()
            .collect()
        } else {
            stmt.query_map(params![fts_q, top_k as i64], |row| {
                let body: String = row.get(3)?;
                let mtime_ms: i64 = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    body,
                    mtime_ms,
                ))
            })?
            .flatten()
            .collect()
        };

        let mut out: Vec<SearchHit> = rows
            .into_iter()
            .map(|(path, snippet, bm25_score, _body, mtime_ms)| {
                let decay = time_decay(mtime_ms, self.time_decay_lambda);
                // BM25 scores are negative (more negative = worse).
                // Normalize: higher bm25_score (less negative) is better.
                // Apply time decay as an additive boost.
                let adjusted_score = bm25_score + self.time_decay_alpha * decay;
                let suspicious =
                    crate::safety::looks_like_prompt_injection(&strip_snippet_markers(&snippet));
                SearchHit {
                    path,
                    snippet,
                    score: adjusted_score,
                    suspicious,
                }
            })
            .collect();

        out.sort_by(|a, b| b.score.total_cmp(&a.score));

        Ok(out)
    }

    /// Deep search: include cold files too.
    pub fn search_deep(&self, query: &str, top_k: usize) -> Result<Vec<SearchHit>> {
        self.search(query, top_k, false)
    }

    /// Compact the index: mark old, never-accessed files as cold and
    /// remove them from the FTS index. Returns the number of files compacted.
    ///
    /// Cold criteria: `access_count == 0 AND age > cold_after_days`.
    /// Files with `access_count > 0` are never compacted (warm protection).
    pub fn compact(&mut self, cold_after_days: u64) -> Result<usize> {
        let now_ms: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let cutoff_ms = now_ms - (cold_after_days as i64 * 86_400_000);

        let tx = self.conn.transaction()?;

        // Find files eligible for cold marking.
        let cold_paths: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT path FROM files WHERE access_count = 0 \
                 AND mtime_ms < ?1 AND is_cold = 0",
            )?;
            let rows = stmt.query_map(params![cutoff_ms], |r| r.get::<_, String>(0))?;
            rows.flatten().collect()
        };

        // Mark them as cold.
        {
            let mut stmt = tx.prepare("UPDATE files SET is_cold = 1 WHERE path = ?1")?;
            for path in &cold_paths {
                let _ = stmt.execute(params![path]);
            }
        }

        tx.commit()?;
        Ok(cold_paths.len())
    }

    /// Return counts of warm vs cold files.
    pub fn warm_cold_counts(&self) -> Result<(usize, usize)> {
        let warm: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM files WHERE is_cold = 0", [], |r| {
                    r.get(0)
                })?;
        let cold: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM files WHERE is_cold = 1", [], |r| {
                    r.get(0)
                })?;
        Ok((warm as usize, cold as usize))
    }

    /// Detect potential conflicts: search for files similar to the given
    /// text and return those with BM25 score above the threshold.
    pub fn detect_conflicts(&self, text: &str, threshold: f64) -> Result<Vec<(String, f64)>> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let fts_q = sanitize_fts_query(text);
        if fts_q.is_empty() {
            return Ok(Vec::new());
        }

        // Search excluding cold and superseded files.
        let sql = r#"
            SELECT f.path, bm25(files_fts) AS rank
            FROM files_fts
            JOIN files f ON f.rowid = files_fts.rowid
            WHERE files_fts MATCH ?1 AND f.is_cold = 0 AND f.is_superseded = 0
            ORDER BY rank
            LIMIT 5
        "#;
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![fts_q], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;

        let results: Vec<(String, f64)> = rows
            .flatten()
            .filter(|(_, score)| *score >= threshold)
            .collect();

        Ok(results)
    }

    /// Mark a file as superseded by another file. The superseded file
    /// remains on disk but is excluded from normal search.
    pub fn supersede(&mut self, old_path: &str, new_id: &str) -> Result<()> {
        // Update the database flag.
        self.conn.execute(
            "UPDATE files SET is_superseded = 1 WHERE path = ?1",
            params![old_path],
        )?;

        // Update the frontmatter in the file on disk.
        // This is best-effort — the DB flag is the authoritative source.
        // mount_root is derived from the db path (not an env var), and we
        // canonicalize before checking containment to guard against path
        // traversal via `..` segments.
        if !self.mount_root.as_os_str().is_empty() {
            let file_path = self.mount_root.join(old_path);
            // Resolve symlinks and `..` before checking containment.
            let canonical = file_path.canonicalize().unwrap_or(file_path.clone());
            if canonical.starts_with(&self.mount_root) && canonical.is_file() {
                let _ = add_superseded_frontmatter(&canonical, new_id);
            }
        }

        Ok(())
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
    /// by descending similarity with time decay boost.
    pub fn search_vec(&self, query_vec: &[f32], top_k: usize) -> Result<Vec<(String, f64)>> {
        let q_norm = l2_normalise(query_vec);

        // JOIN with files to get mtime in a single query (avoids N+1).
        let mut stmt = self.conn.prepare(
            "SELECT v.path, v.embedding, f.mtime_ms \
             FROM files_vec v LEFT JOIN files f ON f.path = v.path",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;

        let mut scores: Vec<(String, f64)> = Vec::new();
        for row in rows {
            let (path, blob, mtime_opt) = match row {
                Ok(r) => r,
                Err(_) => continue,
            };
            let stored = blob_to_f32(&blob);
            if stored.len() != q_norm.len() {
                continue;
            }
            let similarity = dot_product(&q_norm, &stored) as f64;
            // Filter non-finite scores (from zero-norm or degenerate embeddings).
            if !similarity.is_finite() {
                tracing::debug!("skipping non-finite similarity for {path}: {similarity}");
                continue;
            }
            let decay = time_decay(mtime_opt.unwrap_or(0), self.time_decay_lambda);
            let adjusted = similarity * (1.0 + self.time_decay_alpha * decay);
            scores.push((path, adjusted));
        }

        // total_cmp is well-defined and gives a deterministic order even for
        // edge-case values (subnormals, -0.0).
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
        self.search_hybrid_inner(query, query_vec, top_k, self.exclude_cold_on_search)
    }

    /// Hybrid search with explicit cold control.
    pub fn search_hybrid_with_cold(
        &self,
        query: &str,
        query_vec: &[f32],
        top_k: usize,
        exclude_cold: bool,
    ) -> Result<Vec<SearchHit>> {
        self.search_hybrid_inner(query, query_vec, top_k, exclude_cold)
    }

    fn search_hybrid_inner(
        &self,
        query: &str,
        query_vec: &[f32],
        top_k: usize,
        exclude_cold: bool,
    ) -> Result<Vec<SearchHit>> {
        // Run both search strategies.
        let bm25_hits = self.search(query, top_k * 2, exclude_cold);
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
        let mut rrf: std::collections::HashMap<String, (f64, i64)> =
            std::collections::HashMap::new(); // (rrf_score, mtime_ms)
        let mut snippets: std::collections::HashMap<String, (String, bool)> =
            std::collections::HashMap::new();

        for (rank, hit) in bm25_hits.iter().enumerate() {
            let rrf_score = 1.0 / (RRF_K + (rank as f64 + 1.0));
            let entry = rrf.entry(hit.path.clone()).or_insert((0.0, 0));
            entry.0 += rrf_score;
            if entry.1 == 0 {
                // BM25 hits don't have mtime; look it up.
                entry.1 = self.mtime_for(&hit.path).unwrap_or(0);
            }
            snippets
                .entry(hit.path.clone())
                .or_insert((hit.snippet.clone(), hit.suspicious));
        }
        for (rank, (path, _)) in vec_hits.iter().enumerate() {
            let rrf_score = 1.0 / (RRF_K + (rank as f64 + 1.0));
            let entry = rrf.entry(path.clone()).or_insert((0.0, 0));
            entry.0 += rrf_score;
            if entry.1 == 0 {
                entry.1 = self.mtime_for(path).unwrap_or(0);
            }
        }

        // Apply time decay to each merged result.
        let mut merged: Vec<(String, f64)> = rrf
            .into_iter()
            .map(|(path, (rrf_score, mtime_ms))| {
                let decay = time_decay(mtime_ms, self.time_decay_lambda);
                let final_score = rrf_score + self.time_decay_alpha * decay;
                (path, final_score)
            })
            .collect();
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

/// Strip FTS5 snippet highlight markers («, », …) so that prompt-injection
/// detection runs against the cleaned text rather than the decorated snippet.
fn strip_snippet_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '«' | '»' | '…' => {}
            other => out.push(other),
        }
    }
    out
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

/// Compute exponential time decay: `exp(-lambda * age_days)`.
/// Returns 1.0 for very recent files, approaching 0 for old files.
/// When `lambda` is 0, always returns 1.0 (no decay).
pub(crate) fn time_decay(mtime_ms: i64, lambda: f64) -> f64 {
    if lambda == 0.0 {
        return 1.0;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let age_days = ((now_ms - mtime_ms).max(0) as f64) / 86_400_000.0;
    (-lambda * age_days).exp()
}

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

/// Add `superseded_by` to the frontmatter of an existing markdown file.
fn add_superseded_frontmatter(path: &std::path::Path, new_id: &str) -> std::io::Result<()> {
    let content = std::fs::read_to_string(path)?;
    if content.contains("superseded_by:") {
        return Ok(()); // Already superseded.
    }
    // Insert superseded_by after the first --- line if it exists.
    if let Some(pos) = content.find("---\n") {
        let after_first = pos + 4;
        if let Some(second_pos) = content[after_first..].find("---\n") {
            // Insert before the closing ---.
            let insert_point = after_first + second_pos;
            let new_content = format!(
                "{}superseded_by: {}\n{}",
                &content[..insert_point],
                new_id,
                &content[insert_point..]
            );
            std::fs::write(path, new_content)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_search_remove_roundtrip() {
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert("notes/a.md", 100, 10, "rust loves ownership", None)
            .unwrap();
        s.upsert("notes/b.md", 100, 10, "python uses gc", None)
            .unwrap();

        let hits = s.search("rust", 5, true).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "notes/a.md");

        s.remove("notes/a.md").unwrap();
        let hits = s.search("rust", 5, true).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn search_handles_chinese() {
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert("a.md", 0, 0, "你好世界 hello", None).unwrap();
        let hits = s.search("hello", 5, true).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn empty_query_errors() {
        let s = BM25Store::open_in_memory().unwrap();
        assert!(matches!(
            s.search("   ", 5, true),
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
        s.upsert("notes/a.md", 0, 0, "alpha", None).unwrap();
        s.upsert("notes/sub/b.md", 0, 0, "beta", None).unwrap();
        s.upsert("other/c.md", 0, 0, "gamma", None).unwrap();

        let existed = s.remove("notes").unwrap();
        assert!(existed, "removing a populated prefix must report true");

        let paths = s.known_paths().unwrap();
        assert_eq!(paths, vec!["other/c.md".to_string()]);
        // FTS row for the cascaded body is also gone.
        let hits = s.search("alpha", 5, true).unwrap();
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
            let mut s = BM25Store::open_for_test(path).unwrap();
            s.upsert("a.md", 1, 1, "x", None).unwrap();
        }
        // Second open must succeed and preserve data.
        let s = BM25Store::open_for_test(path).unwrap();
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
        let err_msg = match BM25Store::open_for_test(path) {
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
        s.upsert("doc.md", 1, 5, "alpha", None).unwrap();
        // Re-upsert with new body; FTS row should match the new body.
        s.upsert("doc.md", 2, 5, "omega", None).unwrap();
        let hits = s.search("omega", 5, true).unwrap();
        assert_eq!(hits.len(), 1);
        let hits = s.search("alpha", 5, true).unwrap();
        assert!(hits.is_empty(), "old FTS body should be gone");
    }

    #[test]
    fn time_decay_function() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // Recent file (0 days old) → decay ≈ 1.0
        let recent = super::time_decay(now_ms, 0.01);
        assert!(recent > 0.99);

        // 7 days old → exp(-0.01 * 7) ≈ 0.93
        let week_old = super::time_decay(now_ms - 7 * 86_400_000, 0.01);
        assert!((week_old - 0.932).abs() < 0.01);

        // 69 days old (half-life) → exp(-0.01 * 69) ≈ 0.50
        let half_life = super::time_decay(now_ms - 69 * 86_400_000, 0.01);
        assert!((half_life - 0.50).abs() < 0.02);

        // 365 days old → exp(-0.01 * 365) ≈ 0.026
        let year_old = super::time_decay(now_ms - 365 * 86_400_000, 0.01);
        assert!(year_old < 0.03);

        // Lambda = 0 → no decay, always 1.0
        let no_decay = super::time_decay(now_ms - 1000 * 86_400_000, 0.0);
        assert_eq!(no_decay, 1.0);
    }

    #[test]
    fn search_ranks_recent_higher() {
        // Two files with the same content but different mtimes.
        // The more recent one should rank higher (less negative bm25 + decay boost).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut s = BM25Store::open_in_memory_with(0.01, 0.3, true).unwrap();
        // Old file: mtime 100 days ago
        s.upsert(
            "old.md",
            now_ms - 100 * 86_400_000,
            20,
            "rust ownership rules",
            None,
        )
        .unwrap();
        // New file: mtime just now
        s.upsert("new.md", now_ms, 20, "rust ownership rules", None)
            .unwrap();

        let hits = s.search("rust", 5, true).unwrap();
        assert_eq!(hits.len(), 2);
        // The new file should rank higher (higher score = less negative + decay boost)
        assert_eq!(hits[0].path, "new.md");
        assert_eq!(hits[1].path, "old.md");
    }

    #[test]
    fn search_no_decay_behaves_same() {
        // With lambda=0, time decay is disabled — all files get decay=1.0.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut s = BM25Store::open_in_memory_with(0.0, 0.0, true).unwrap();
        s.upsert(
            "old.md",
            now_ms - 365 * 86_400_000,
            20,
            "ancient rust facts",
            None,
        )
        .unwrap();
        s.upsert("new.md", now_ms, 20, "modern python tricks", None)
            .unwrap();

        // Both searches should still work; the scores just don't discriminate by time.
        let hits1 = s.search("ancient", 5, true).unwrap();
        assert_eq!(hits1.len(), 1);
        assert_eq!(hits1[0].path, "old.md");
        let hits2 = s.search("python", 5, true).unwrap();
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].path, "new.md");
    }

    #[test]
    fn compact_marks_old_files_cold() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut s = BM25Store::open_in_memory_with(0.01, 0.3, true).unwrap();
        // Fresh file (5 days old, access_count=0)
        s.upsert(
            "fresh.md",
            now_ms - 5 * 86_400_000,
            20,
            "recent knowledge",
            None,
        )
        .unwrap();
        // Old file (60 days old, access_count=0)
        s.upsert(
            "old.md",
            now_ms - 60 * 86_400_000,
            20,
            "ancient wisdom",
            None,
        )
        .unwrap();

        // Compact with 30-day threshold.
        let compacted = s.compact(30).unwrap();
        assert_eq!(compacted, 1); // only old.md should be compacted

        // Normal search should not see old.md (excluded by is_cold filter).
        let hits = s.search("ancient", 5, true).unwrap();
        assert!(
            hits.is_empty(),
            "cold file should not appear in normal search"
        );

        // Deep search should still find it.
        let hits = s.search("ancient", 5, false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "old.md");

        // Fresh file should still be visible.
        let hits = s.search("recent", 5, true).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "fresh.md");
    }

    #[test]
    fn warm_cold_counts() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut s = BM25Store::open_in_memory_with(0.01, 0.3, true).unwrap();
        s.upsert("warm.md", now_ms, 20, "warm content", None)
            .unwrap();
        s.upsert("old1.md", now_ms - 50 * 86_400_000, 20, "old1", None)
            .unwrap();
        s.upsert("old2.md", now_ms - 100 * 86_400_000, 20, "old2", None)
            .unwrap();

        let (warm, cold) = s.warm_cold_counts().unwrap();
        assert_eq!(warm, 3);
        assert_eq!(cold, 0);

        s.compact(30).unwrap();
        let (warm, cold) = s.warm_cold_counts().unwrap();
        assert_eq!(warm, 1); // warm.md
        assert_eq!(cold, 2); // old1.md + old2.md
    }

    #[test]
    fn compact_excludes_cold_from_normal_search() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut s = BM25Store::open_in_memory_with(0.01, 0.3, true).unwrap();
        s.upsert(
            "old.md",
            now_ms - 50 * 86_400_000,
            20,
            "very old unique keyword",
            None,
        )
        .unwrap();

        // Before compact: visible in search.
        let hits = s.search("unique", 5, true).unwrap();
        assert_eq!(hits.len(), 1);

        // Compact.
        s.compact(30).unwrap();

        // After compact: not visible in normal search (cold filter).
        let hits = s.search("unique", 5, true).unwrap();
        assert!(hits.is_empty());

        // But visible in deep search.
        let hits = s.search("unique", 5, false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "old.md");
    }

    #[test]
    fn detect_conflicts_finds_similar_files() {
        let mut s = BM25Store::open_in_memory_with(0.01, 0.3, true).unwrap();
        s.upsert("user-pref.md", 100, 50, "用户偏好 rust 系统编程", None)
            .unwrap();

        // Search for similar content (shares key terms).
        let conflicts = s.detect_conflicts("用户偏好 rust", -2.0).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, "user-pref.md");
    }

    #[test]
    fn detect_conflicts_no_match() {
        let mut s = BM25Store::open_in_memory_with(0.01, 0.3, true).unwrap();
        s.upsert("cooking.md", 100, 50, "如何制作意大利面", None)
            .unwrap();

        // Unrelated query should not match.
        let conflicts = s.detect_conflicts("rust ownership rules", -2.0).unwrap();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn superseded_files_excluded_from_search() {
        let mut s = BM25Store::open_in_memory_with(0.01, 0.3, true).unwrap();
        s.upsert("old.md", 100, 50, "unique old content here", None)
            .unwrap();
        s.upsert("new.md", 200, 50, "unique new content here", None)
            .unwrap();

        // Both visible before superseding.
        let hits = s.search("unique", 10, true).unwrap();
        assert_eq!(hits.len(), 2);

        // Supersede old file.
        s.supersede("old.md", "new-id").unwrap();

        // Only new file visible in search.
        let hits = s.search("unique", 10, true).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "new.md");
    }

    #[test]
    fn agent_scope_isolated_only_returns_own_memories() {
        let mut s = BM25Store::open_in_memory().unwrap();
        s.upsert(
            "a/own.md",
            100,
            10,
            "agent alpha owns this note",
            Some("alpha"),
        )
        .unwrap();
        s.upsert(
            "a/shared.md",
            100,
            10,
            "agent alpha note tagged alpha",
            Some("alpha"),
        )
        .unwrap();
        s.upsert(
            "b/other.md",
            100,
            10,
            "agent beta owns this note",
            Some("beta"),
        )
        .unwrap();
        s.upsert("u/legacy.md", 100, 10, "agent alpha legacy note", None)
            .unwrap();

        // isolated:alpha sees ONLY alpha's own — not beta, not legacy.
        let hits = s
            .search_scoped("agent alpha", 10, true, Some("isolated:alpha"))
            .unwrap();
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths.len(), 2, "isolated:alpha should see 2 own memories");
        assert!(paths.iter().all(|p| p.starts_with("a/")));

        // filter:alpha sees alpha's own + unscoped (NULL) — legacy included,
        // but never beta's.
        let hits = s
            .search_scoped("agent alpha", 10, true, Some("filter:alpha"))
            .unwrap();
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert!(paths.contains(&"u/legacy.md"));
        assert!(!paths.iter().any(|p| p.starts_with("b/")));
    }

    #[test]
    fn agent_scope_rejects_invalid_agent_id() {
        let s = BM25Store::open_in_memory().unwrap();
        // SQL-meta chars and path separators must be refused, not silently
        // widened to shared mode.
        for bad in ["isolated:foo'bar", "filter:a;b", "isolated:with/slash"] {
            let err = s
                .search_scoped("agent alpha", 10, true, Some(bad))
                .unwrap_err();
            assert!(
                matches!(err, MemoryError::InvalidArgument(_)),
                "expected InvalidArgument for {bad:?}, got {err:?}"
            );
        }
    }
}
