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

use super::connection::{create_connection, default_base_path};

// Re-export public types from sub-modules
pub use events::TraceEventDetail;
pub use pending::{PendingCallInfo, SseEnrichment};
pub use session::{SavingsSessionSummary, SessionSummary, ToolCallTurnInfo, TraceSummary};
pub use stats::{AgentTokenSummary, ModelTimeseriesBucket, TimeseriesBucket};

/// SQLite-backed GenAI event storage
pub struct GenAISqliteStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

impl GenAISqliteStore {
    /// Create a new GenAI SQLite store at the default path
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Self::default_path();
        Self::new_with_path(&path)
    }

    /// Create a new GenAI SQLite store at an arbitrary path
    pub fn new_with_path(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let conn = create_connection(path)?;
        let store = GenAISqliteStore {
            conn: Mutex::new(conn),
            db_path: path.to_path_buf(),
        };
        store.init_tables()?;

        // Log current database size on startup
        let current_size = store.get_total_db_size();
        let max_size = schema::get_max_db_size();
        let threshold = schema::get_prune_threshold();
        log::info!(
            "GenAISqliteStore initialized: db_size={}MB, threshold={}MB, max={}MB",
            current_size / 1024 / 1024,
            threshold / 1024 / 1024,
            max_size / 1024 / 1024
        );

        Ok(store)
    }

    /// Default database path
    pub fn default_path() -> PathBuf {
        default_base_path().join("genai_events.db")
    }
}
