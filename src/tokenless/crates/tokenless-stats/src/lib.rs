//! Tokenless Statistics Library
//!
//! Tracks compression metrics (characters, tokens, text content)
//! for Agent hook integrations. Records before/after data for
//! schema compression, response compression, and command rewriting.

pub mod config;
pub mod query;
pub mod record;
pub mod recorder;
pub mod tokenizer;

pub use record::{OperationType, StatsRecord};

pub use recorder::{StatsError, StatsRecorder, StatsResult, StatsSummary};

pub use query::{format_list, format_show, format_summary};

pub use tokenizer::{Tokenizer, count_chars, estimate_tokens, estimate_tokens_from_chars};

pub use config::TokenlessConfig;

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
