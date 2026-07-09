//! GenAI semantic events SQLite storage
//!
//! Stores GenAI events (LLM calls, tool uses, etc.) to SQLite when SLS is not configured.
//! Implements the GenAIExporter trait for pluggable integration.
//!
//! # Size Limit
//!
//! The database size can be configured via `AGENTSIGHT_GENAI_DB_MAX_SIZE_MB` environment
//! variable (default: 200 MB). When approaching 90% of the limit, old records are pruned
//! automatically. The size check includes the main database file plus WAL and SHM files.

mod events;
mod pending;
mod schema;
mod session;
mod stats;
#[cfg(test)]
mod tests;

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::connection::{create_connection, default_base_path};
use crate::config::BatchConfig;

// Re-export public types from sub-modules
pub use events::TraceEventDetail;
pub use pending::{PendingCallInfo, PendingOrigin, SseEnrichment};
pub use session::{SavingsSessionSummary, SessionSummary, ToolCallTurnInfo, TraceSummary};
pub use stats::{AgentTokenSummary, ModelTimeseriesBucket, TimeseriesBucket};

/// SQLite-backed GenAI event storage
pub struct GenAISqliteStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    /// Batch insert configuration: events are buffered until `max_size` or
    /// `flush_ms` is reached, then written inside a single SQLite transaction.
    batch_config: BatchConfig,
    /// Buffered events waiting to be flushed.
    pending: Mutex<Vec<crate::genai::semantic::GenAISemanticEvent>>,
    /// Timestamp of the last successful flush.
    last_flush: Mutex<Instant>,
}

impl GenAISqliteStore {
    /// Create a new GenAI SQLite store at the default path
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Self::default_path();
        Self::new_with_path(&path)
    }

    /// Create a new GenAI SQLite store at an arbitrary path with default batch config.
    pub fn new_with_path(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_path_and_batch(path, None)
    }

    /// Create a new GenAI SQLite store with explicit batch configuration.
    pub fn new_with_path_and_batch(
        path: &std::path::Path,
        batch: Option<BatchConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let conn = create_connection(path)?;
        let batch_config = batch.unwrap_or_default();
        let store = GenAISqliteStore {
            conn: Mutex::new(conn),
            db_path: path.to_path_buf(),
            batch_config,
            pending: Mutex::new(Vec::new()),
            last_flush: Mutex::new(Instant::now()),
        };
        store.init_tables()?;

        // Log current database size on startup
        let current_size = store.get_total_db_size();
        let max_size = schema::get_max_db_size();
        let threshold = schema::get_prune_threshold();
        log::info!(
            "GenAISqliteStore initialized: db_size={}MB, threshold={}MB, max={}MB, batch_max_size={}, batch_flush_ms={}",
            current_size / 1024 / 1024,
            threshold / 1024 / 1024,
            max_size / 1024 / 1024,
            store.batch_config.max_size,
            store.batch_config.flush_ms,
        );

        Ok(store)
    }

    /// Flush any buffered events to SQLite.
    ///
    /// Events are written through `store_event` which handles prune/retry.
    /// The batch value comes from reducing the number of flush calls (fewer
    /// fsync/WAL checkpoints), not from wrapping in a single transaction
    /// (which would require refactoring the Mutex-based conn access).
    pub fn flush(&self) {
        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() {
            return;
        }
        let events: Vec<_> = pending.drain(..).collect();
        drop(pending); // release lock before writing

        let mut ok_count = 0usize;
        for event in &events {
            if let Err(e) = self.store_event(event) {
                log::warn!("Failed to store GenAI event in batch flush: {e}");
            } else {
                ok_count += 1;
            }
        }
        if ok_count > 0 {
            log::debug!("Batch-flushed {ok_count} GenAI events");
        }
        *self.last_flush.lock().unwrap() = Instant::now();
    }

    /// Check if batch flush is needed based on size or time.
    fn should_flush(&self, pending_len: usize) -> bool {
        if pending_len >= self.batch_config.max_size {
            return true;
        }
        let elapsed = self.last_flush.lock().unwrap().elapsed();
        elapsed >= Duration::from_millis(self.batch_config.flush_ms as u64)
    }

    /// Default database path
    pub fn default_path() -> PathBuf {
        default_base_path().join("genai_events.db")
    }
}

impl Drop for GenAISqliteStore {
    fn drop(&mut self) {
        self.flush();
    }
}
