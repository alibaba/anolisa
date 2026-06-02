//! AgentSight - AI Agent observability library
//!
//! This crate provides eBPF-based observability for AI agents, including:
//! - SSL/TLS traffic capture and parsing
//! - HTTP request/response aggregation
//! - LLM token usage tracking
//! - Process lifecycle monitoring
//!
//! # Architecture
//!
//! ```text
//! probes → parser → aggregator → analyzer → storage
//!   ↓         ↓          ↓           ↓         ↓
//! Event  ParsedMessage  AggregatedResult  AnalysisResult  持久化
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use agentsight::{AgentSight, AgentsightConfig};
//!
//! let config = AgentsightConfig::new();
//! let mut sight = AgentSight::new(config)?;  // auto-attaches and starts polling
//! sight.run()?;  // blocking event loop
//! ```

// Crate-wide clippy allows for lints that are either subjective style choices or
// reflect intentional design here, so we can enforce `clippy -D warnings` in CI
// without churning these:
// - type_complexity: a few SQLite row-tuple / callback signatures are clearer
//   inline than behind a type alias.
// - large_enum_variant: event/result enums carry one large variant by design;
//   boxing it would pessimize the common path and complicate call sites.
// - too_many_arguments: a couple of SQL insert helpers mirror their table columns.
// - module_inception: `probes::probes` is the established layout.
// - should_implement_trait: an inherent `from_str` that is not the FromStr trait.
#![allow(
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::too_many_arguments,
    clippy::module_inception,
    clippy::should_implement_trait
)]

pub mod config;
pub mod probes;

// Re-export config types
pub use config::{AgentsightConfig, default_base_path};
pub mod aggregator;
pub mod analyzer;
pub mod atif;
pub mod chrome_trace;
pub mod discovery;
pub mod event;
pub mod ffi;
pub mod genai;
pub mod health;
pub mod interruption;
pub mod parser;
pub mod response_map;
#[cfg(feature = "server")]
pub mod server;
pub mod skill_metrics;
pub mod storage;
pub mod tokenizer;
mod unified;

// Re-export common types for convenience
pub use aggregator::{
    AggregatedProcess, AggregatedResponse, AggregatedResult, Aggregator, ConnectionId,
    ConnectionState, HttpConnectionAggregator, HttpPair, ProcessEventAggregator,
};
pub use analyzer::{
    AnalysisResult, Analyzer, AnthropicMessage, AnthropicRequest, AnthropicResponse,
    AnthropicUsage, AuditAnalyzer, AuditEventType, AuditExtra, AuditRecord, AuditSummary,
    HttpRecord, LLMProvider, MessageParser, MessageRole, OpenAIChatMessage, OpenAIContent,
    OpenAIRequest, OpenAIResponse, OpenAIUsage, ParsedApiMessage, PromptTokenCount, TokenParser,
    TokenRecord, TokenUsage,
};
pub use chrome_trace::{ChromeTraceEvent, ToChromeTraceEvent, TraceArgs, next_flow_id, ns_to_us};
pub use parser::{
    Http2FrameType, Http2Parser, HttpParser, ParseResult, ParsedHttp2Frame, ParsedHttpMessage,
    ParsedMessage, ParsedProcEvent, ParsedRequest, ParsedResponse, ParsedSseEvent, Parser,
    ProcEventType, ProcTraceParser, SseParser,
};
pub use storage::{
    AuditStore, HttpStore, SqliteConfig, SqliteStore, Storage, StorageBackend, TimePeriod,
    TokenBreakdown, TokenComparison, TokenQuery, TokenQueryResult, TokenStore, Trend,
    format_tokens, format_tokens_with_commas,
};

// Re-export unified entry point
pub use unified::AgentSight;

// Re-export file watch types
pub use probes::FileWatchEvent;

// Re-export response mapping
pub use response_map::ResponseSessionMapper;

// Re-export discovery types
pub use config::default_cmdline_rules;
pub use discovery::{AgentInfo, AgentScanner, CmdlineGlobMatcher, DiscoveredAgent, ProcessContext};

// Re-export genai types
pub use genai::{
    AgentInteraction, GenAIBuilder, GenAIExporter, GenAISemanticEvent, GenAIStore, GenAIStoreStats,
    InputMessage, LLMCall, LLMRequest, LLMResponse, LogtailExporter, MessagePart, OutputMessage,
    StreamChunk, ToolDefinition, ToolUse,
};
