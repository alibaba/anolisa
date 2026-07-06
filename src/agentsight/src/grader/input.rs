//! Snapshot loading and stable input hashing for grader runs.

use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::types::{GraderError, RULE_GRADER_VERSION, TargetType};
use crate::storage::sqlite::genai::TraceEventDetail;
use crate::storage::sqlite::{GenAISqliteStore, InterruptionRecord, InterruptionStore};

/// Evidence snapshot used by a grader run.
pub struct EvaluationInput {
    /// Evaluated target kind.
    pub target_type: TargetType,
    /// Evaluated conversation id.
    pub target_id: String,
    /// Captured LLM call rows for the conversation.
    pub events: Vec<TraceEventDetail>,
    /// Captured interruption rows for the conversation.
    pub interruptions: Vec<InterruptionRecord>,
    /// Stable hash over the evaluated snapshot.
    pub input_hash: String,
    /// True when the snapshot contains pending calls and was forced.
    pub evaluated_with_pending: bool,
    /// Number of pending LLM calls in the snapshot.
    pub pending_call_count: usize,
}

/// Load a conversation snapshot and compute its stable input hash.
pub fn load_conversation_input(
    storage_path: &Path,
    conversation_id: &str,
    force: bool,
) -> Result<EvaluationInput, GraderError> {
    let genai_store = GenAISqliteStore::new_with_path(storage_path)
        .map_err(|error| GraderError::Storage(error.to_string()))?;
    let events = genai_store
        .get_events_by_conversation(conversation_id)
        .map_err(|error| GraderError::Storage(error.to_string()))?;

    if events.is_empty() {
        return Err(GraderError::ConversationNotFound(
            conversation_id.to_string(),
        ));
    }

    let pending_call_count = events
        .iter()
        .filter(|event| event.status.as_deref() == Some("pending"))
        .count();
    if pending_call_count > 0 && !force {
        return Err(GraderError::ConversationNotReady {
            pending_count: pending_call_count,
        });
    }

    let interruptions = load_conversation_interruptions(storage_path, conversation_id)?;
    let input_hash = compute_input_hash(conversation_id, &events, &interruptions)?;

    Ok(EvaluationInput {
        target_type: TargetType::Conversation,
        target_id: conversation_id.to_string(),
        events,
        interruptions,
        input_hash,
        evaluated_with_pending: pending_call_count > 0,
        pending_call_count,
    })
}

fn load_conversation_interruptions(
    storage_path: &Path,
    conversation_id: &str,
) -> Result<Vec<InterruptionRecord>, GraderError> {
    if storage_path == Path::new(":memory:") {
        return Ok(Vec::new());
    }
    let parent = storage_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let interruption_path = parent.join("interruption_events.db");
    let store = InterruptionStore::new_with_path(&interruption_path)
        .map_err(|error| GraderError::Storage(error.to_string()))?;
    store
        .list_by_conversation(conversation_id)
        .map_err(|error| GraderError::Storage(error.to_string()))
}

fn compute_input_hash(
    conversation_id: &str,
    events: &[TraceEventDetail],
    interruptions: &[InterruptionRecord],
) -> Result<String, GraderError> {
    #[derive(Serialize)]
    struct HashPayload<'a> {
        schema: &'static str,
        grader_version: &'static str,
        conversation_id: &'a str,
        events: Vec<serde_json::Value>,
        interruptions: Vec<serde_json::Value>,
    }

    let payload = HashPayload {
        schema: "agentsight-grader-input-v1",
        grader_version: RULE_GRADER_VERSION,
        conversation_id,
        events: events.iter().map(event_hash_value).collect(),
        interruptions: interruptions.iter().map(interruption_hash_value).collect(),
    };
    let bytes = serde_json::to_vec(&payload)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn event_hash_value(event: &TraceEventDetail) -> serde_json::Value {
    serde_json::json!({
        "id": event.id,
        "call_id": &event.call_id,
        "start_timestamp_ns": event.start_timestamp_ns,
        "end_timestamp_ns": &event.end_timestamp_ns,
        "model": &event.model,
        "input_tokens": event.input_tokens,
        "output_tokens": event.output_tokens,
        "total_tokens": event.total_tokens,
        "input_messages": &event.input_messages,
        "output_messages": &event.output_messages,
        "system_instructions": &event.system_instructions,
        "agent_name": &event.agent_name,
        "process_name": &event.process_name,
        "pid": &event.pid,
        "user_query": &event.user_query,
        "event_json": &event.event_json,
        "trace_id": &event.trace_id,
        "conversation_id": &event.conversation_id,
        "cache_read_tokens": &event.cache_read_tokens,
        "status": &event.status,
        "interruption_type": &event.interruption_type,
    })
}

fn interruption_hash_value(record: &InterruptionRecord) -> serde_json::Value {
    serde_json::json!({
        "interruption_id": &record.interruption_id,
        "session_id": &record.session_id,
        "trace_id": &record.trace_id,
        "conversation_id": &record.conversation_id,
        "call_id": &record.call_id,
        "pid": &record.pid,
        "agent_name": &record.agent_name,
        "interruption_type": &record.interruption_type,
        "severity": &record.severity,
        "occurred_at_ns": record.occurred_at_ns,
        "detail": &record.detail,
        "resolved": record.resolved,
    })
}
