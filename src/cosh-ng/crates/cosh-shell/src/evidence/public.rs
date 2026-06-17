pub mod context_window;
mod redaction;

pub use context_window::{
    build_context_window, build_related_history_index, context_blocks_from_entries,
    format_context_prompt, provider_safe_command_fact_line, provider_safe_command_facts,
    redact_provider_command_text, terminal_output_id, ContextEntry, ContextWindowConfig,
    ProviderCommandFacts, RelatedHistoryConfig,
};
