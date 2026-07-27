//! Tokenless Statistics Library
//!
//! Tracks compression metrics (characters, tokens, text content)
//! for Agent hook integrations. Records before/after data for
//! schema compression, response compression, and command rewriting.

pub mod config;
pub mod home;
pub mod query;
pub mod record;
pub mod recorder;
pub mod sls;
pub mod tokenizer;

pub use record::{CompressionMode, OperationType, StatsRecord};

pub use recorder::{StatsError, StatsRecorder, StatsResult, StatsSummary};

pub use query::{
    format_compare, format_compare_json, format_list, format_show, format_summary,
    format_summary_json,
};

pub use tokenizer::{Tokenizer, count_chars, estimate_tokens, estimate_tokens_from_bytes};

pub use config::TokenlessConfig;

pub use home::get_home_dir;

pub use sls::{SlsRecord, SlsWriter};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
