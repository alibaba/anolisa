//! Schema initialization, migrations, and size limit management for GenAI SQLite store.

use rusqlite::params;

use super::GenAISqliteStore;

// ─── Size limit configuration ──────────────────────────────────────────────────

/// Environment variable name for max database size in MB
const ENV_MAX_DB_SIZE_MB: &str = "AGENTSIGHT_GENAI_DB_MAX_SIZE_MB";
/// Default max database size: 200 MB
const DEFAULT_MAX_DB_SIZE_MB: u64 = 200;
/// Percentage of records to prune per attempt
const PRUNE_PERCENT: f64 = 0.05;
/// Maximum prune retry attempts to avoid infinite loop
pub(super) const MAX_PRUNE_RETRIES: u32 = 3;

/// Get max database size from environment variable or use default
pub(super) fn get_max_db_size() -> u64 {
    std::env::var(ENV_MAX_DB_SIZE_MB)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_DB_SIZE_MB)
        * 1024
        * 1024
}

/// Get prune threshold (90% of max)
pub(super) fn get_prune_threshold() -> u64 {
    (get_max_db_size() as f64 * 0.9) as u64
}

impl GenAISqliteStore {
    /// Initialize database tables
    pub(super) fn init_tables(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS genai_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                -- call lifecycle status: 'pending' | 'complete' | 'interrupted'
                -- 'pending'     : request captured, waiting for response
                -- 'complete'    : full request+response recorded
                -- 'interrupted' : response never arrived (crash / truncation)
                status TEXT NOT NULL DEFAULT 'complete',
                call_id TEXT,
                trace_id TEXT,
                conversation_id TEXT,
                session_id TEXT,
                instance TEXT,
                start_timestamp_ns INTEGER NOT NULL,
                end_timestamp_ns INTEGER,
                duration_ns INTEGER,
                first_output_timestamp_ns INTEGER,
                pid INTEGER,
                process_name TEXT,
                agent_name TEXT,
                -- GenAI standard fields
                operation_name TEXT,
                provider TEXT,
                model TEXT,
                request_model TEXT,
                response_model TEXT,
                temperature REAL,
                max_tokens INTEGER,
                top_p REAL,
                frequency_penalty REAL,
                presence_penalty REAL,
                finish_reasons TEXT,
                server_address TEXT,
                -- Token usage
                input_tokens INTEGER,
                output_tokens INTEGER,
                total_tokens INTEGER,
                cache_creation_tokens INTEGER,
                cache_read_tokens INTEGER,
                -- Messages (JSON)
                system_instructions TEXT,
                input_messages TEXT,
                output_messages TEXT,
                -- AgentSight extensions
                user_query TEXT,
                http_method TEXT,
                http_path TEXT,
                status_code INTEGER,
                is_sse INTEGER,
                sse_event_count INTEGER,
                -- Interruption type detected for this call (nullable)
                interruption_type TEXT,
                -- Call kind classification (main/recap/web_search)
                call_kind TEXT NOT NULL DEFAULT 'main',
                -- Pending row provenance and reconciliation key
                pending_origin TEXT NOT NULL DEFAULT 'request_capture',
                pending_match_key TEXT,
                -- Full event as JSON (fallback)
                event_json TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_genai_session_id ON genai_events(session_id);
            CREATE INDEX IF NOT EXISTS idx_genai_trace_id ON genai_events(trace_id);
            CREATE INDEX IF NOT EXISTS idx_genai_conversation_id ON genai_events(conversation_id);
            CREATE INDEX IF NOT EXISTS idx_genai_instance ON genai_events(instance);
            CREATE INDEX IF NOT EXISTS idx_genai_start_timestamp ON genai_events(start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_pid ON genai_events(pid);
            CREATE INDEX IF NOT EXISTS idx_genai_model ON genai_events(model);
            CREATE INDEX IF NOT EXISTS idx_genai_call_id ON genai_events(call_id);
            CREATE INDEX IF NOT EXISTS idx_genai_provider ON genai_events(provider);
            -- Composite indexes for common query patterns
            CREATE INDEX IF NOT EXISTS idx_genai_session_timestamp ON genai_events(session_id, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_trace_timestamp ON genai_events(trace_id, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_conversation_timestamp ON genai_events(conversation_id, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_pid_timestamp ON genai_events(pid, start_timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_genai_instance_timestamp ON genai_events(instance, start_timestamp_ns)",
            // NOTE: idx_genai_status and idx_genai_interruption_type are NOT created here
            // because they depend on columns added via migration. They are created in the
            // migration blocks below, which guarantees the columns exist first.
        )?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_resource_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ns INTEGER NOT NULL,
                pid INTEGER NOT NULL,
                agent_name TEXT,
                cpu_percent REAL NOT NULL,
                memory_bytes INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_resource_pid_timestamp
                ON agent_resource_samples(pid, timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_resource_timestamp
                ON agent_resource_samples(timestamp_ns);",
        )?;

        // ── Forward-compatible migrations ──────────────────────────────────────
        // Each block checks for a column's existence before ALTER TABLE, making
        // all migrations idempotent and safe to run on both old and new databases.
        // Columns are listed in the order they were added historically.

        // Query existing columns once to avoid repeated PRAGMA calls
        let existing_cols: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('genai_events')")?;
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect()
        };

        // Helper macro: ALTER TABLE only if column absent, then always ensure index
        macro_rules! ensure_col {
            // Column with no index
            ($col:literal, $def:literal) => {
                if !existing_cols.contains($col) {
                    conn.execute_batch(&format!(
                        "ALTER TABLE genai_events ADD COLUMN {} {};",
                        $col, $def
                    ))?;
                    log::info!("Migrated genai_events: added '{}' column", $col);
                }
            };
            // Column + index
            ($col:literal, $def:literal, $idx:literal) => {
                if !existing_cols.contains($col) {
                    conn.execute_batch(&format!(
                        "ALTER TABLE genai_events ADD COLUMN {} {};",
                        $col, $def
                    ))?;
                    log::info!("Migrated genai_events: added '{}' column", $col);
                }
                // Always run CREATE INDEX IF NOT EXISTS — safe even if index already exists
                conn.execute_batch(&format!(
                    "CREATE INDEX IF NOT EXISTS {} ON genai_events({});",
                    $idx, $col
                ))?;
            };
        }

        // v2: Anthropic prompt-cache token counters
        ensure_col!("cache_creation_tokens", "INTEGER");
        ensure_col!("cache_read_tokens", "INTEGER");

        // v3: two-phase write lifecycle status
        ensure_col!(
            "status",
            "TEXT NOT NULL DEFAULT 'complete'",
            "idx_genai_status"
        );

        // v4: per-call interruption type
        ensure_col!("interruption_type", "TEXT", "idx_genai_interruption_type");

        // Migration: add conversation_id column for existing databases
        let _ = conn.execute(
            "ALTER TABLE genai_events ADD COLUMN conversation_id TEXT",
            [],
        );

        // v5: tool_call_ids JSON array for output tool calls
        ensure_col!("tool_call_ids", "TEXT");

        // v6: call_kind classification (main/recap/web_search)
        ensure_col!(
            "call_kind",
            "TEXT NOT NULL DEFAULT 'main'",
            "idx_genai_call_kind"
        );

        // v7: pending provenance for idle/drain lifecycle handling
        ensure_col!(
            "pending_origin",
            "TEXT NOT NULL DEFAULT 'request_capture'",
            "idx_genai_pending_origin"
        );

        // v8: stable key used to reconcile idle stream snapshots on completion
        ensure_col!("pending_match_key", "TEXT", "idx_genai_pending_match_key");

        // v9: first provider event that carries model output
        ensure_col!("first_output_timestamp_ns", "INTEGER");

        Ok(())
    }

    // ─── Size limit methods ───────────────────────────────────────────────────

    /// Get total database size (main db + wal + shm)
    pub(super) fn get_total_db_size(&self) -> u64 {
        let mut total = 0u64;

        // Main database file
        if let Ok(meta) = std::fs::metadata(&self.db_path) {
            total += meta.len();
        }

        // WAL file
        let wal_path = format!("{}-wal", self.db_path.display());
        if let Ok(meta) = std::fs::metadata(&wal_path) {
            total += meta.len();
        }

        // SHM file
        let shm_path = format!("{}-shm", self.db_path.display());
        if let Ok(meta) = std::fs::metadata(&shm_path) {
            total += meta.len();
        }

        total
    }

    /// Logical data size: physical size minus freelist pages.
    ///
    /// Purge convergence is measured on this: deleting rows does not shrink
    /// the file without VACUUM — freed pages go to the freelist and are
    /// reused by future inserts, so the physical file stabilizes at its
    /// historical peak while the logical size reflects live data.
    pub(super) fn effective_db_size(&self) -> u64 {
        let physical = self.get_total_db_size();
        let free_bytes = {
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            let freelist: i64 = match conn.query_row("PRAGMA freelist_count", [], |r| r.get(0)) {
                Ok(v) => v,
                Err(e) => {
                    // Fall back to the physical size (conservative: the prune
                    // loop may over-delete by the freelist amount) — but make
                    // the degraded measurement visible.
                    log::warn!(
                        "freelist_count query failed; logical size falls back \
                         to physical size: {e}"
                    );
                    0
                }
            };
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |r| r.get(0))
                .unwrap_or(4096);
            (freelist.max(0) * page_size.max(0)) as u64
        };
        physical.saturating_sub(free_bytes)
    }

    /// Check database size and prune if approaching limit.
    ///
    /// Uses adaptive pruning: the fraction of records deleted per iteration
    /// scales with how far the database exceeds the configured maximum, so a
    /// severely oversized database is brought back under control quickly
    /// instead of inching down 5% at a time.
    ///
    /// The loop never runs VACUUM: rebuilding the whole file would push its
    /// pages through the page cache, which counts against the service's
    /// cgroup memory limit and can OOM-kill the process on large databases
    /// (#2888). Deleting rows + a truncating WAL checkpoint is enough — the
    /// freelist is reused by future inserts, so the physical file stops
    /// growing once the logical size fits.
    pub(super) fn check_and_prune_if_needed(&self) -> Result<(), Box<dyn std::error::Error>> {
        let physical_size = self.get_total_db_size();
        let threshold = get_prune_threshold();

        // Trigger on physical size (disk safety is physical), converge on
        // logical size (deletes only shrink the logical size via freelist).
        if physical_size < threshold {
            return Ok(());
        }

        let mut current_size = self.effective_db_size();
        if current_size < threshold {
            // Physically large but mostly freelist: future writes reuse free
            // pages, no rows need to be deleted.
            return Ok(());
        }

        let max_size = get_max_db_size();
        let overshoot = current_size as f64 / max_size as f64;

        // Delete a larger fraction when the database is far over the limit.
        //   1–2× over → 10% per iteration
        //   2–5× over → 25% per iteration
        //   5×+  over → 50% per iteration
        let prune_pct = if overshoot > 5.0 {
            0.50
        } else if overshoot > 2.0 {
            0.25
        } else {
            0.10
        };

        log::info!(
            "Database size {}MB exceeding threshold {}MB (overshoot {:.1}×), \
             pruning {:.0}% per iteration",
            current_size / 1024 / 1024,
            threshold / 1024 / 1024,
            overshoot,
            prune_pct * 100.0
        );

        const MAX_ITERATIONS: u32 = 20;
        let mut iterations = 0u32;

        while current_size >= threshold && iterations < MAX_ITERATIONS {
            iterations += 1;

            if let Err(e) = self.prune_old_records_with_percent(prune_pct) {
                log::warn!("Prune failed on iteration {iterations}: {e}");
                break;
            }

            // Flush and truncate the WAL so freed pages are visible and the
            // WAL does not grow unbounded. Never VACUUM here (#2888). A busy
            // checkpoint (another connection holds a read snapshot) leaves the
            // WAL intact — stop pruning: further deletes would keep appending
            // WAL frames and never converge.
            match self.wal_checkpoint() {
                Ok(true) => {
                    log::warn!(
                        "WAL checkpoint busy on iteration {iterations}; \
                         stopping prune (the WAL could not be truncated)"
                    );
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    log::warn!("WAL checkpoint failed on iteration {iterations}: {e}");
                }
            }

            let new_size = self.effective_db_size();
            current_size = new_size;

            if current_size >= threshold {
                log::info!(
                    "Database still {}MB (threshold {}MB), continue pruning \
                     (iteration {iterations}/{MAX_ITERATIONS})",
                    current_size / 1024 / 1024,
                    threshold / 1024 / 1024,
                );
            }
        }

        if current_size >= threshold {
            log::warn!(
                "Database size {}MB still above threshold {}MB after pruning; \
                 will retry on next write",
                current_size / 1024 / 1024,
                threshold / 1024 / 1024,
            );
        } else {
            log::info!(
                "Pruning complete, database size now {}MB",
                current_size / 1024 / 1024
            );
        }

        Ok(())
    }

    /// Prune old records using the default 5% ratio.
    ///
    /// Thin wrapper around [`prune_old_records_with_percent`] for callers that
    /// only need the conservative default (e.g. SQLITE_FULL retry).
    pub(super) fn prune_old_records(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.prune_old_records_with_percent(PRUNE_PERCENT)
    }

    /// Delete the oldest `percent` fraction of records, ordered by id.
    ///
    /// `percent` is clamped to \[0.0, 1.0\]. At least one record is deleted
    /// when the table is non-empty and `percent` > 0.
    fn prune_old_records_with_percent(
        &self,
        percent: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pct = percent.clamp(0.0, 1.0);
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let event_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM genai_events", [], |row| row.get(0))?;
        let resource_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM agent_resource_samples", [], |row| {
                row.get(0)
            })?;

        if event_count == 0 && resource_count == 0 {
            return Ok(());
        }

        let event_delete_count = if event_count > 0 {
            ((event_count as f64) * pct).max(1.0) as i64
        } else {
            0
        };
        let resource_delete_count = if resource_count > 0 {
            ((resource_count as f64) * pct).max(1.0) as i64
        } else {
            0
        };

        log::info!(
            "Pruning {event_delete_count}/{event_count} GenAI events and \
             {resource_delete_count}/{resource_count} resource samples ({:.1}%)",
            pct * 100.0
        );

        let deleted_events = conn.execute(
            "DELETE FROM genai_events WHERE id IN (
                SELECT id FROM genai_events ORDER BY id ASC LIMIT ?1
            )",
            params![event_delete_count],
        )?;
        let deleted_resources = conn.execute(
            "DELETE FROM agent_resource_samples WHERE id IN (
                SELECT id FROM agent_resource_samples ORDER BY id ASC LIMIT ?1
            )",
            params![resource_delete_count],
        )?;

        log::info!(
            "Deleted {deleted_events} GenAI events and {deleted_resources} resource samples"
        );

        Ok(())
    }

    /// Flush WAL frames to the main database and truncate the WAL file.
    ///
    /// Call during graceful shutdown to clean up `-wal` / `-shm` files —
    /// mirrors the sibling stores (token, http, audit) which do this via
    /// `connection::wal_checkpoint` in their own `checkpoint()` methods.
    ///
    /// Returns `Ok(true)` when the checkpoint was blocked by another
    /// connection's read snapshot (busy): the statement succeeds but the WAL
    /// is NOT truncated. Purge loops must stop deleting in that case — the
    /// WAL stays in the size measurement while deletes keep appending frames,
    /// so the loop never converges (#2888).
    pub fn wal_checkpoint(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let busy: i32 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        Ok(busy != 0)
    }
}
