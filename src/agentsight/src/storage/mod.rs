//! Storage module - unified persistence layer
//!
//! This module provides storage abstraction with multiple backend support:
//! - SQLite: Local file-based storage (current implementation)
//! - SLS: Alibaba Cloud Log Service (planned)
//!
//! Use `Storage` for a unified interface that combines all storage types.

pub mod sqlite;
mod unified;

// Re-export from sqlite module
pub use sqlite::{
    // Audit storage
    AuditStore,
    // HTTP storage
    HttpStore,
    SqliteStore,
    TimePeriod,
    TokenBreakdown,
    TokenComparison,
    TokenConsumptionFilter,
    TokenConsumptionQueryResult,
    TokenConsumptionRecord,
    // Token consumption storage
    TokenConsumptionStore,
    TokenQuery,
    TokenQueryResult,
    // Token storage
    TokenStore,
    Trend,
    // Connection utilities
    create_connection,
    default_base_path,
    format_tokens,
    format_tokens_with_commas,
};

// Re-export unified storage
pub use unified::{SqliteConfig, Storage, StorageBackend};
