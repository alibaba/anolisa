//! Unified Storage - high-level entry point for persistence
//!
//! This module provides a unified interface for storing and querying records.
//! It supports multiple storage backends with a common API.
//!
//! # Architecture
//!
//! ```text
//! storage/
//! ├── mod.rs           # Module declarations and re-exports
//! ├── unified.rs       # Unified Storage facade
//! ├── sqlite/          # SQLite implementation
//! │   ├── mod.rs
//! │   ├── audit.rs     # AuditStore implementation
//! │   ├── token.rs     # TokenStore implementation
//! │   └── connection.rs
//! └── sls/             # SLS implementation (planned)
//!     └── ...
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use agentsight::storage::Storage;
//! use agentsight::analyzer::AnalysisResult;
//!
//! // Create default SQLite storage
//! let storage = Storage::sqlite()?;
//!
//! // Store analysis result (automatically routes to correct store)
//! storage.store(&analysis_result)?;
//!
//! // Or access specific stores directly
//! storage.audit().insert(&audit_record)?;
//! storage.token().add(token_record)?;
//! ```

use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::sqlite::connection::default_base_path;
use super::sqlite::{AuditStore, HttpStore, TokenConsumptionStore, TokenStore};
use crate::analyzer::AnalysisResult;

/// Storage backend type
#[derive(Debug, Clone, Default)]
pub enum StorageBackend {
    /// SQLite local storage
    #[default]
    Sqlite,
    /// Alibaba Cloud Log Service (planned)
    Sls {
        endpoint: String,
        project: String,
        logstore: String,
        access_key_id: String,
        access_key_secret: String,
    },
    /// No-op backend: stores nothing, used when all persistence features are disabled.
    Noop,
    // Future: other backends can be added here
}

/// Configuration for SQLite storage
#[derive(Debug, Clone)]
pub struct SqliteConfig {
    /// Base directory for database files
    pub base_path: PathBuf,
    /// Database filename (shared for all tables)
    pub db_name: String,
    /// Audit table name
    pub audit_table: String,
    /// Token table name
    pub token_table: String,
    /// HTTP table name
    pub http_table: String,
    /// Token consumption breakdown table name
    pub token_consumption_table: String,
    /// Data retention period in days (0 = no limit)
    pub retention_days: u64,
    /// Auto-purge check interval (every N inserts, 0 = disabled)
    pub purge_interval: u64,
    /// Max database file size in MB (0 = no size-based limit)
    pub max_db_size_mb: u64,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            base_path: default_base_path(),
            db_name: "agentsight.db".to_string(),
            audit_table: "audit_events".to_string(),
            token_table: "token_records".to_string(),
            http_table: "http_records".to_string(),
            token_consumption_table: "token_consumption".to_string(),
            retention_days: 30,
            purge_interval: 100000,
            max_db_size_mb: 500,
        }
    }
}

impl SqliteConfig {
    /// Create a new SQLite config with custom base path
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            base_path,
            ..Default::default()
        }
    }

    /// Get database path
    pub fn db_path(&self) -> PathBuf {
        self.base_path.join(&self.db_name)
    }
}

/// Unified storage facade that provides access to all storage types
///
/// This is the main entry point for storage operations, supporting:
/// - Audit record persistence
/// - Token usage persistence and querying
/// - HTTP request/response persistence
/// - Multiple backend support (SQLite, SLS, etc.)
pub struct Storage {
    backend: StorageBackend,
    audit_store: AuditStore,
    token_store: TokenStore,
    http_store: HttpStore,
    token_consumption_store: TokenConsumptionStore,
    /// Data retention period in days (0 = no limit)
    retention_days: u64,
    /// Auto-purge check interval (every N inserts, 0 = disabled)
    purge_interval: u64,
    /// Max database file size in bytes (0 = no size-based limit)
    max_db_size_bytes: u64,
    /// Path to the SQLite database file (for size checking)
    db_path: PathBuf,
    /// Insert counter for auto-purge triggering
    insert_count: AtomicU64,
}

impl Storage {
    /// Create a new Storage with the specified backend
    pub fn new(backend: StorageBackend) -> Result<Self> {
        match &backend {
            StorageBackend::Sqlite => {
                let config = SqliteConfig::default();
                Self::with_sqlite_config(&config)
            }
            StorageBackend::Sls { .. } => {
                // TODO: Implement SLS storage
                anyhow::bail!("SLS storage backend is not yet implemented");
            }
            StorageBackend::Noop => Ok(Self::noop()),
        }
    }

    /// Create a new Storage with SQLite backend and custom config
    pub fn with_sqlite_config(config: &SqliteConfig) -> Result<Self> {
        let db_path = config.db_path();
        let audit_store = AuditStore::with_table(&db_path, &config.audit_table)?;
        let token_store = TokenStore::with_table(&db_path, &config.token_table);
        let http_store = HttpStore::with_table(&db_path, &config.http_table)?;
        let token_consumption_store =
            TokenConsumptionStore::with_table(&db_path, &config.token_consumption_table)?;
        let storage = Storage {
            backend: StorageBackend::Sqlite,
            audit_store,
            token_store,
            http_store,
            token_consumption_store,
            retention_days: config.retention_days,
            purge_interval: config.purge_interval,
            max_db_size_bytes: config.max_db_size_mb * 1024 * 1024,
            db_path,
            insert_count: AtomicU64::new(0),
        };

        // If the database is already oversized on startup (e.g. from a prior
        // crash or a config change), prune immediately rather than waiting for
        // the next purge interval.
        if storage.max_db_size_bytes > 0 {
            if let Err(e) = storage.purge_oversized() {
                log::warn!("Startup size-based cleanup failed: {e}");
            }
        }

        Ok(storage)
    }

    /// Create a new Storage with default SQLite config
    pub fn sqlite() -> Result<Self> {
        Self::new(StorageBackend::Sqlite)
    }

    /// Create a new no-op Storage that silently drops all writes.
    ///
    /// Used when all persistence features are disabled in `agentsight.json`.
    pub fn noop() -> Self {
        // Reuse SQLite stores with an in-memory database so the store API
        // remains available without touching the filesystem.
        let db_path = PathBuf::from(":memory:");
        let audit_store = AuditStore::with_table(&db_path, "audit_events")
            .expect("in-memory audit store should always succeed");
        let token_store = TokenStore::with_table(&db_path, "token_records");
        let http_store = HttpStore::with_table(&db_path, "http_records")
            .expect("in-memory http store should always succeed");
        let token_consumption_store =
            TokenConsumptionStore::with_table(&db_path, "token_consumption")
                .expect("in-memory token_consumption store should always succeed");
        Storage {
            backend: StorageBackend::Noop,
            audit_store,
            token_store,
            http_store,
            token_consumption_store,
            retention_days: 0,
            purge_interval: 0,
            max_db_size_bytes: 0,
            db_path,
            insert_count: AtomicU64::new(0),
        }
    }

    /// Returns true if this storage backend is the no-op backend.
    pub fn is_noop(&self) -> bool {
        matches!(self.backend, StorageBackend::Noop)
    }

    /// Get the backend type
    pub fn backend(&self) -> &StorageBackend {
        &self.backend
    }

    /// Get audit storage
    pub fn audit(&self) -> &AuditStore {
        &self.audit_store
    }

    /// Get token storage
    pub fn token(&self) -> &TokenStore {
        &self.token_store
    }

    /// Get HTTP storage
    pub fn http(&self) -> &HttpStore {
        &self.http_store
    }

    /// Get token consumption breakdown storage
    pub fn token_consumption(&self) -> &TokenConsumptionStore {
        &self.token_consumption_store
    }

    /// Store an analysis result (automatically routes to correct store)
    ///
    /// This is the primary method for persisting analysis results.
    /// It automatically dispatches to the appropriate store based on the result type.
    /// Periodically triggers data purge based on `purge_interval` configuration.
    pub fn store(&self, result: &AnalysisResult) -> Result<i64> {
        if let AnalysisResult::Http(_) = result {
            return Ok(0);
        }
        if matches!(self.backend, StorageBackend::Noop) {
            log::trace!("Noop storage dropping analysis result");
            return Ok(0);
        }
        log::debug!("Storing analysis result: {result:?}");
        let id = match result {
            AnalysisResult::Audit(record) => self.audit_store.insert(record),
            AnalysisResult::Token(record) => self.token_store.insert(record),
            AnalysisResult::Message(_msg) => {
                log::trace!("Message storage not implemented, skipping");
                Ok(0)
            }
            AnalysisResult::PromptTokens(_count) => {
                log::trace!("Prompt token count storage not implemented, skipping");
                Ok(0)
            }
            AnalysisResult::Http(record) => self.http_store.insert(record),
            AnalysisResult::TokenConsumption(breakdown) => self.token_consumption_store.insert(
                breakdown,
                breakdown.timestamp_ns,
                breakdown.pid,
                &breakdown.comm,
            ),
        }?;

        // Auto-purge check: trigger every `purge_interval` inserts
        if self.purge_interval > 0 {
            let count = self.insert_count.fetch_add(1, Ordering::Relaxed) + 1;
            if count.is_multiple_of(self.purge_interval) {
                if self.retention_days > 0 {
                    if let Err(e) = self.purge_expired() {
                        log::warn!("Auto-purge (age-based) failed: {e}");
                    }
                }
                if self.max_db_size_bytes > 0 {
                    if let Err(e) = self.purge_oversized() {
                        log::warn!("Auto-purge (size-based) failed: {e}");
                    }
                }
            }
        }

        Ok(id)
    }

    /// Purge records older than the configured retention period
    ///
    /// Deletes rows from all tables where `timestamp_ns` is older than
    /// `now - retention_days`. Returns the total number of deleted rows.
    ///
    /// This is called automatically by `store()` every `purge_interval` inserts,
    /// but can also be called manually.
    pub fn purge_expired(&self) -> Result<u64> {
        if self.retention_days == 0 {
            return Ok(0);
        }

        let cutoff_ns = Self::retention_cutoff_ns(self.retention_days);
        let mut total_deleted = 0u64;

        let audit_deleted = self.audit_store.purge_before(cutoff_ns)?;
        total_deleted += audit_deleted;

        let token_deleted = self.token_store.purge_before(cutoff_ns)?;
        total_deleted += token_deleted;

        let http_deleted = self.http_store.purge_before(cutoff_ns)?;
        total_deleted += http_deleted;

        let consumption_deleted = self.token_consumption_store.purge_before(cutoff_ns)?;
        total_deleted += consumption_deleted;

        if total_deleted > 0 {
            log::info!(
                "Purged {} expired records (retention={}d, audit={}, token={}, http={}, consumption={})",
                total_deleted,
                self.retention_days,
                audit_deleted,
                token_deleted,
                http_deleted,
                consumption_deleted,
            );

            // Reclaim free pages after bulk deletes. VACUUM writes through
            // the WAL, so a TRUNCATE checkpoint is needed for the file to
            // actually shrink. Both are best-effort: freed pages are reusable
            // by future inserts even when they fail.
            if let Err(e) = self.audit_store.vacuum() {
                log::warn!("VACUUM after age-based purge failed: {e}");
            }
            if let Err(e) = self.audit_store.checkpoint() {
                log::warn!("WAL checkpoint after age-based purge failed: {e}");
            }
        }

        Ok(total_deleted)
    }

    /// Purge oldest records when the database exceeds the size limit.
    ///
    /// Size accounting includes the main file plus the `-wal` and `-shm`
    /// companions — in WAL mode a large share of recent data lives in the WAL
    /// file, so checking the main file alone would under-report.
    ///
    /// Work is organized in rounds: each round deletes a fraction of every
    /// table's oldest rows, then runs one VACUUM + checkpoint before
    /// re-measuring. VACUUM rewrites the whole file, so it must not run per
    /// batch — bounding it per round keeps a multi-GB cleanup tractable.
    ///
    /// Rounds continue until the size is within the limit. A round that does
    /// not shrink the file is not necessarily a failure (deleting small rows
    /// may free no page), so the bite doubles while the file stands still;
    /// the purge stops early only when VACUUM/checkpoint itself keeps failing
    /// or after 20 rounds, both reported as errors.
    ///
    /// Called automatically by `store()` during purge checks and by
    /// `with_sqlite_config()` on startup.
    pub fn purge_oversized(&self) -> Result<()> {
        if self.max_db_size_bytes == 0 {
            return Ok(());
        }

        let mut round = 0u32;
        let mut reclaim_failures = 0u32;
        // Rows and bytes can decouple (B-tree page sharing): deleting many
        // small rows may free no page at all, so a non-shrinking round is
        // normal and triggers a bigger bite next round instead of a stall.
        let mut pct_boost = 1.0f64;

        loop {
            let size = self.total_db_size();
            if size <= self.max_db_size_bytes {
                break;
            }

            if round >= 20 {
                log::error!(
                    "Size-based purge did not converge after {round} rounds: database is \
                     {size} bytes (limit {}). Size enforcement is suspended — manual \
                     intervention required (free disk space or remove the database).",
                    self.max_db_size_bytes
                );
                break;
            }
            round += 1;

            // Delete a fraction of each table's rows per round and re-measure
            // after the round's VACUUM+checkpoint. A fixed per-batch row
            // count without re-measuring between batches wipes any table
            // smaller than the per-round batch total (#2870). Bigger bites
            // when far over the limit; mirrors the genai store's prune policy.
            let overshoot = size as f64 / self.max_db_size_bytes as f64;
            let base_pct = if overshoot > 5.0 {
                0.50
            } else if overshoot > 2.0 {
                0.25
            } else {
                0.10
            };
            let pct = (base_pct * pct_boost).min(0.9);

            let mut round_deleted = 0usize;
            round_deleted += self
                .audit_store
                .delete_oldest_batch(purge_share(self.audit_store.count()?, pct))?;
            round_deleted += self
                .token_store
                .delete_oldest_batch(purge_share(self.token_store.count(), pct))?;
            round_deleted += self
                .http_store
                .delete_oldest_batch(purge_share(self.http_store.count()?, pct))?;
            round_deleted += self
                .token_consumption_store
                .delete_oldest_batch(purge_share(self.token_consumption_store.count(), pct))?;

            if round_deleted == 0 {
                break; // Nothing left to delete
            }

            // VACUUM first, then checkpoint: in WAL mode VACUUM writes the
            // rebuilt database through the WAL, so the file only shrinks once
            // a TRUNCATE checkpoint flushes it back. Failures are the real
            // stall signal (e.g. disk full): deleting further rows would
            // destroy data without freeing space, so stop after repeated
            // failures and escalate.
            let mut reclaim_ok = true;
            if let Err(e) = self.audit_store.vacuum() {
                log::warn!("VACUUM during size-based purge failed: {e}");
                reclaim_ok = false;
            }
            if let Err(e) = self.audit_store.checkpoint() {
                log::warn!("WAL checkpoint during size-based purge failed: {e}");
                reclaim_ok = false;
            }
            if reclaim_ok {
                reclaim_failures = 0;
            } else {
                reclaim_failures += 1;
                if reclaim_failures >= 3 {
                    log::error!(
                        "Size-based purge suspended after {reclaim_failures} failed \
                         VACUUM/checkpoint rounds at {size} bytes (limit {}). Freed \
                         pages remain reusable by future inserts; free disk space or \
                         remove the database to enforce the cap.",
                        self.max_db_size_bytes
                    );
                    break;
                }
            }

            let new_size = self.total_db_size();
            log::info!(
                "Size-based purge round {round}: deleted {round_deleted} rows, \
                 database now {new_size} bytes (limit {})",
                self.max_db_size_bytes
            );

            if new_size < size {
                pct_boost = 1.0;
            } else {
                // Deletion made no dent in the file (page sharing). Double the
                // bite so subsequent rounds reach the rows that dominate the
                // file instead of stalling on small-row deletions.
                pct_boost = (pct_boost * 2.0).min(9.0);
            }
        }

        Ok(())
    }

    /// Total on-disk size of the database: main file + `-wal` + `-shm`.
    ///
    /// A missing companion file is normal (e.g. after a TRUNCATE checkpoint)
    /// and counts as zero; any other metadata error is logged because it can
    /// under-report the size and mask an oversized database.
    fn total_db_size(&self) -> u64 {
        let len = |path: &str| match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => {
                log::warn!("Cannot stat {path} for size accounting, assuming 0 bytes: {e}");
                0
            }
        };
        len(&self.db_path.display().to_string())
            + len(&format!("{}-wal", self.db_path.display()))
            + len(&format!("{}-shm", self.db_path.display()))
    }

    /// Compute the cutoff timestamp for retention
    fn retention_cutoff_ns(retention_days: u64) -> u64 {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let retention_ns = retention_days * 24 * 3600 * 1_000_000_000;
        now_ns.saturating_sub(retention_ns)
    }

    /// Store multiple analysis results
    ///
    /// Returns the number of successfully stored records.
    pub fn store_all(&self, results: &[AnalysisResult]) -> Result<usize> {
        let mut count = 0;
        for result in results {
            self.store(result)?;
            count += 1;
        }
        Ok(count)
    }

    /// Execute WAL checkpoint on all store connections.
    ///
    /// Flushes WAL data back to the main database file and truncates the
    /// `-wal` / `-shm` files. Should be called during graceful shutdown.
    ///
    /// Since all stores share the same database file, a successful checkpoint
    /// on any one connection covers the entire database. We try all connections
    /// and report the first error (if any).
    pub fn checkpoint(&self) -> Result<()> {
        // Only need one successful checkpoint since all stores share the same db,
        // but we try on audit_store first and fall through if it fails.
        if let Err(e) = self.audit_store.checkpoint() {
            log::warn!("Audit store checkpoint failed: {e}, trying token store");
            if let Err(e2) = self.token_store.checkpoint() {
                log::warn!("Token store checkpoint failed: {e2}, trying http store");
                self.http_store.checkpoint()?;
            }
        }
        log::info!("WAL checkpoint completed");
        Ok(())
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        if let Err(e) = self.checkpoint() {
            log::warn!("WAL checkpoint during Storage drop failed: {e}");
        }
    }
}

/// Rows to delete from a table in one purge round: `pct` of its rows, at
/// least 1 (deleting from an empty table is a no-op).
fn purge_share(rows: u64, pct: f64) -> usize {
    ((rows as f64 * pct) as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::token::TokenRecord;

    #[test]
    fn test_noop_storage_is_noop() {
        let storage = Storage::noop();
        assert!(storage.is_noop());
    }

    #[test]
    fn test_noop_storage_store_returns_zero() {
        let storage = Storage::noop();
        assert!(storage.is_noop());
        // Call store to verify noop path returns Ok(0) without writing
        let token_record = crate::analyzer::token::TokenRecord {
            id: 0,
            timestamp_ns: 0,
            pid: 1,
            comm: "test".to_string(),
            agent: None,
            model: None,
            provider: "test".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            request_id: None,
            endpoint: None,
            tool_calls: vec![],
            reasoning_content: None,
        };
        let result = crate::analyzer::AnalysisResult::Token(token_record);
        let id = storage.store(&result).unwrap();
        assert_eq!(id, 0);
    }

    #[test]
    fn test_noop_storage_should_persist() {
        let storage = Storage::noop();
        // Just verify it doesn't panic
        let _ = storage.is_noop();
        drop(storage);
    }

    /// Unique per-test directory under the system temp dir.
    fn unique_base_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentsight_unified_{label}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_config(base_path: PathBuf, max_db_size_mb: u64) -> SqliteConfig {
        SqliteConfig {
            base_path,
            // Disable both auto-purge paths so tests trigger purges explicitly.
            retention_days: 0,
            purge_interval: 0,
            max_db_size_mb,
            ..Default::default()
        }
    }

    /// Token record with a payload of roughly `payload_bytes` for growing the
    /// database file quickly in size-limit tests.
    fn bulky_token_record(timestamp_ns: u64, payload_bytes: usize) -> TokenRecord {
        TokenRecord {
            id: 0,
            timestamp_ns,
            pid: 1,
            comm: "test".to_string(),
            agent: None,
            model: None,
            provider: "test".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            request_id: None,
            // `endpoint` is a persisted TEXT column, so the payload actually
            // lands in the database file (`reasoning_content` is not stored).
            endpoint: Some("x".repeat(payload_bytes)),
            tool_calls: vec![],
            reasoning_content: None,
        }
    }

    #[test]
    fn test_purge_oversized_disabled_when_limit_zero() {
        let dir = unique_base_dir("limit_zero");
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), 0)).unwrap();

        for i in 0..10 {
            storage
                .token_store
                .insert(&bulky_token_record(i, 64))
                .unwrap();
        }
        storage.purge_oversized().unwrap();

        assert_eq!(storage.token_store.count(), 10, "limit 0 must never delete");
        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_purge_oversized_noop_when_below_limit() {
        let dir = unique_base_dir("below_limit");
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), 500)).unwrap();

        for i in 0..10 {
            storage
                .token_store
                .insert(&bulky_token_record(i, 64))
                .unwrap();
        }
        storage.purge_oversized().unwrap();

        assert_eq!(
            storage.token_store.count(),
            10,
            "below-limit purge must not delete"
        );
        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_purge_oversized_shrinks_db_below_limit() {
        let dir = unique_base_dir("shrink");
        let limit_mb = 1u64;
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), limit_mb)).unwrap();

        // ~3MB of data: 300 rows × 10KB payload, well over the 1MB limit.
        for i in 0..300 {
            storage
                .token_store
                .insert(&bulky_token_record(i, 10 * 1024))
                .unwrap();
        }
        storage.checkpoint().unwrap();
        assert!(
            storage.total_db_size() > limit_mb * 1024 * 1024,
            "test setup must produce an oversized database"
        );

        storage.purge_oversized().unwrap();

        assert!(
            storage.total_db_size() <= limit_mb * 1024 * 1024,
            "database must be within the size limit after purge, got {} bytes",
            storage.total_db_size()
        );
        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for #2870: a database only slightly over the limit must be
    /// trimmed oldest-first, not wiped. The previous fixed-batch loop
    /// (20x1000 rows per table per round, without re-measuring between
    /// batches) emptied every table in the first round; this test fails under
    /// it because nothing would survive.
    #[test]
    fn test_purge_oversized_trims_oldest_keeps_newest() {
        let dir = unique_base_dir("trim_oldest");
        let limit_mb = 1u64;
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), limit_mb)).unwrap();

        // ~1.2MB over a 1MB limit (~17% overshoot): 110 rows x ~10KB payload.
        let base_ts = 1_000_000_000u64;
        for i in 0..110u64 {
            storage
                .token_store
                .insert(&bulky_token_record(base_ts + i, 10 * 1024))
                .unwrap();
        }
        storage.checkpoint().unwrap();
        let size_before = storage.total_db_size();
        assert!(
            size_before > limit_mb * 1024 * 1024,
            "setup must exceed the limit, got {size_before}"
        );

        storage.purge_oversized().unwrap();

        let remaining = storage.token_store.count();
        assert!(remaining > 0, "purge must not wipe the table (#2870)");
        assert!(remaining < 110, "oversized db must be trimmed");
        assert!(
            storage.total_db_size() <= limit_mb * 1024 * 1024,
            "must converge below the limit"
        );

        // Oldest-first trimming removes low timestamps, so the newest seeded
        // row must survive.
        let conn = rusqlite::Connection::open(dir.join("agentsight.db")).unwrap();
        let max_ts: u64 = conn
            .query_row("SELECT MAX(timestamp_ns) FROM token_records", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(max_ts, base_ts + 109, "newest row must survive");
        drop(conn);

        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When the file is dominated by a few large NEW rows and the oldest rows
    /// are tiny, deleting the oldest fraction frees no page (B-tree page
    /// sharing). Treating a non-shrinking round as a stall exits with the cap
    /// still exceeded; the purge must keep trimming until it reaches the rows
    /// that dominate the file.
    #[test]
    fn test_purge_oversized_skewed_row_sizes_still_converges() {
        let dir = unique_base_dir("skewed");
        let limit_mb = 1u64;
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), limit_mb)).unwrap();

        // 100 tiny old rows (~5KB total, sharing B-tree pages) + 4 large new
        // rows (~260KB each): ~1.05MB, slightly over the 1MB limit.
        for i in 0..100u64 {
            storage
                .token_store
                .insert(&bulky_token_record(1_000 + i, 50))
                .unwrap();
        }
        for i in 0..4u64 {
            storage
                .token_store
                .insert(&bulky_token_record(1_000_000 + i, 260 * 1024))
                .unwrap();
        }
        storage.checkpoint().unwrap();
        // Compact once up front so the first purge round's VACUUM cannot hide
        // behind rebuild gains instead of actual deletions.
        storage.audit_store.vacuum().unwrap();
        let size_before = storage.total_db_size();
        assert!(
            size_before > limit_mb * 1024 * 1024,
            "setup must exceed the limit, got {size_before}"
        );

        storage.purge_oversized().unwrap();

        assert!(
            storage.total_db_size() <= limit_mb * 1024 * 1024,
            "must converge below the limit even when early rounds free nothing, got {}",
            storage.total_db_size()
        );
        let conn = rusqlite::Connection::open(dir.join("agentsight.db")).unwrap();
        let max_ts: u64 = conn
            .query_row("SELECT MAX(timestamp_ns) FROM token_records", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(max_ts, 1_000_003, "newest large row must survive");
        drop(conn);

        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_purge_oversized_runs_on_startup() {
        let dir = unique_base_dir("startup");
        let limit_mb = 1u64;

        // Grow the database past the limit with size checks disabled.
        {
            let storage = Storage::with_sqlite_config(&test_config(dir.clone(), 0)).unwrap();
            for i in 0..300 {
                storage
                    .token_store
                    .insert(&bulky_token_record(i, 10 * 1024))
                    .unwrap();
            }
            storage.checkpoint().unwrap();
            assert!(storage.total_db_size() > limit_mb * 1024 * 1024);
        }

        // Reopening with a limit must trigger the startup cleanup.
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), limit_mb)).unwrap();
        assert!(
            storage.total_db_size() <= limit_mb * 1024 * 1024,
            "startup cleanup must bring the database within the limit"
        );
        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_total_db_size_counts_wal_file() {
        let dir = unique_base_dir("wal_size");
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), 500)).unwrap();

        // Without a checkpoint, recent writes stay in the -wal file.
        for i in 0..20 {
            storage
                .token_store
                .insert(&bulky_token_record(i, 1024))
                .unwrap();
        }

        let wal_len = std::fs::metadata(format!("{}-wal", storage.db_path.display()))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(
            wal_len > 0,
            "WAL file should hold the un-checkpointed writes"
        );

        let main_len = std::fs::metadata(&storage.db_path).unwrap().len();
        assert!(
            storage.total_db_size() >= main_len + wal_len,
            "size accounting must include the WAL file"
        );
        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_oldest_batch_removes_oldest_rows() {
        let dir = unique_base_dir("oldest_batch");
        let storage = Storage::with_sqlite_config(&test_config(dir.clone(), 0)).unwrap();

        for ts in 1..=10 {
            storage
                .token_store
                .insert(&bulky_token_record(ts, 64))
                .unwrap();
        }

        let deleted = storage.token_store.delete_oldest_batch(4).unwrap();
        assert_eq!(deleted, 4);

        let remaining = storage.token_store.all();
        assert_eq!(remaining.len(), 6);
        let min_ts = remaining.iter().map(|r| r.timestamp_ns).min().unwrap();
        assert_eq!(min_ts, 5, "the four oldest rows (ts 1-4) must be gone");
        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `store()` must run both purge paths (age-based and size-based) every
    /// `purge_interval` inserts, including the VACUUM after an age purge.
    #[test]
    fn test_store_triggers_auto_purge() {
        let dir = unique_base_dir("auto_purge");
        let config = SqliteConfig {
            base_path: dir.clone(),
            retention_days: 1,
            purge_interval: 1, // purge check on every insert
            max_db_size_mb: 500,
            ..Default::default()
        };
        let storage = Storage::with_sqlite_config(&config).unwrap();

        // ts=1ns is far older than the 1-day retention cutoff: the purge
        // check right after this insert must delete it again.
        let expired = crate::analyzer::AnalysisResult::Token(bulky_token_record(1, 64));
        storage.store(&expired).unwrap();
        assert_eq!(storage.token_store.count(), 0, "expired row must be purged");

        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let fresh = crate::analyzer::AnalysisResult::Token(bulky_token_record(now_ns, 64));
        storage.store(&fresh).unwrap();
        assert_eq!(storage.token_store.count(), 1, "fresh row must survive");
        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
