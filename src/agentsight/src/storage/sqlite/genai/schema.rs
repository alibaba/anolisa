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
        let conn = self.conn.lock().unwrap();
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

    /// Check database size and prune if approaching limit
    ///
    /// Keeps pruning until size drops below threshold to avoid repeated triggers.
    pub(super) fn check_and_prune_if_needed(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut current_size = self.get_total_db_size();
        let threshold = get_prune_threshold();

        if current_size < threshold {
            return Ok(());
        }

        log::info!(
            "Database size {}MB exceeding threshold {}MB, pruning old records",
            current_size / 1024 / 1024,
            threshold / 1024 / 1024
        );

        // Keep pruning until below threshold (max 5 iterations to prevent infinite loop)
        let mut iterations = 0;
        while current_size >= threshold && iterations < 5 {
            iterations += 1;
            self.prune_old_records()?;
            self.checkpoint()?;
            current_size = self.get_total_db_size();

            if current_size >= threshold {
                log::info!(
                    "Database still {}MB (threshold {}MB), continue pruning (iteration {})",
                    current_size / 1024 / 1024,
                    threshold / 1024 / 1024,
                    iterations
                );
            }
        }

        log::info!(
            "Pruning complete, database size now {}MB",
            current_size / 1024 / 1024
        );

        Ok(())
    }

    /// Prune old records to free up space
    ///
    /// Deletes a percentage of oldest records based on id.
    pub(super) fn prune_old_records(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();

        // Get total count
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM genai_events", [], |row| row.get(0))?;

        if count == 0 {
            return Ok(());
        }

        // Calculate how many to delete (5% of total)
        let delete_count = ((count as f64) * PRUNE_PERCENT).max(1.0) as i64;

        log::info!(
            "Pruning {} of {} records ({:.1}%)",
            delete_count,
            count,
            PRUNE_PERCENT * 100.0
        );

        // Delete oldest records by id
        let deleted = conn.execute(
            "DELETE FROM genai_events WHERE id IN (
                SELECT id FROM genai_events ORDER BY id ASC LIMIT ?1
            )",
            params![delete_count],
        )?;

        log::info!("Deleted {deleted} records");

        Ok(())
    }

    /// Execute WAL checkpoint and VACUUM to reclaim disk space
    ///
    /// 1. VACUUM: rebuild database to compact data
    /// 2. Checkpoint: flush and truncate WAL file
    ///
    /// Note: VACUUM in WAL mode creates a new db file, so we need to
    /// re-enable WAL and checkpoint after VACUUM.
    pub(super) fn checkpoint(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();

        // VACUUM rebuilds the database (works better before checkpoint in WAL mode)
        conn.execute_batch("VACUUM;")?;

        // Re-enable WAL mode (VACUUM may reset it)
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        // Checkpoint with TRUNCATE to shrink WAL file
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;

        Ok(())
    }

    /// Flush WAL frames to the main database and truncate the WAL file.
    ///
    /// Call during graceful shutdown to clean up `-wal` / `-shm` files —
    /// mirrors the sibling stores (token, http, audit) which do this via
    /// `connection::wal_checkpoint` in their own `checkpoint()` methods.
    pub fn wal_checkpoint(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}
