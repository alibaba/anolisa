//! GenAI Semantic Module
//!
//! This module provides GenAI-specific semantic conversion and storage
//! for LLM API calls, tool uses, and agent interactions.

pub mod semantic;
pub mod builder;
pub mod exporter;
pub mod storage;
pub mod sls;

pub use semantic::{
    GenAISemanticEvent, LLMCall, LLMRequest, LLMResponse,
    MessagePart, InputMessage, OutputMessage,
    TokenUsage, ToolUse, AgentInteraction, StreamChunk,
    ToolDefinition,
};
pub use exporter::GenAIExporter;
pub use builder::GenAIBuilder;
pub use storage::{GenAIStore, GenAIStoreStats};
pub use sls::SlsUploader;

// Blanket implementation: Arc<T> implements GenAIExporter if T does.
// This allows storing an Arc<GenAISqliteStore> both in genai_exporters and
// as a direct handle for two-phase pending/complete writes.
use std::sync::Arc;
impl<T: GenAIExporter + Sync> GenAIExporter for Arc<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn export(&self, events: &[GenAISemanticEvent]) {
        (**self).export(events);
    }
}
