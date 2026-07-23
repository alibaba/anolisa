//! OpenAI-compatible LLM client used by the LLM judgment layers.

pub mod client;
pub mod types;

pub use client::LlmClient;
pub use types::ChatMessage;
