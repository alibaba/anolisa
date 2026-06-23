//! Event storage and retrieval methods for GenAI SQLite store.

use rusqlite::params;

use super::GenAISqliteStore;
use super::schema::MAX_PRUNE_RETRIES;
use crate::genai::exporter::GenAIExporter;
use crate::genai::semantic::GenAISemanticEvent;

// ─── Query result types ────────────────────────────────────────────────────────

/// One LLM call event within a trace
#[derive(Debug, serde::Serialize)]
pub struct TraceEventDetail {
    pub id: i64,
    pub call_id: Option<String>,
    pub start_timestamp_ns: i64,
    pub end_timestamp_ns: Option<i64>,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    /// JSON string of input messages array
    pub input_messages: Option<String>,
    /// JSON string of output messages array
    pub output_messages: Option<String>,
    /// JSON string of system instructions
    pub system_instructions: Option<String>,
    pub agent_name: Option<String>,
    pub process_name: Option<String>,
    pub pid: Option<i64>,
    /// The user query that triggered this LLM call
    pub user_query: Option<String>,
    /// Raw full event JSON stored at write time — used as fallback when
    /// output_messages is NULL (e.g. SSE streams that weren't fully parsed)
    pub event_json: Option<String>,
    /// Trace ID (LLM API response_id) — needed for session-level ATIF export
    /// to identify individual LLM calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Conversation ID (user query fingerprint) — groups multiple LLM calls
    /// triggered by the same user query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Cache read tokens — maps to ATIF cached_tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    /// Call lifecycle status: 'pending' | 'complete' | 'interrupted'
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Interruption type if abnormal: 'llm_error' | 'sse_truncated' | 'timeout' | etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interruption_type: Option<String>,
}

impl GenAISqliteStore {
    /// Fetch all LLM call events for a given trace ID (response_id).
    pub fn get_trace_events(
        &self,
        trace_id: &str,
    ) -> Result<Vec<TraceEventDetail>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE trace_id = ?1
               AND event_type = 'llm_call'
             ORDER BY start_timestamp_ns ASC",
        )?;
        let rows = stmt.query_map(params![trace_id], |row| {
            Ok(TraceEventDetail {
                id: row.get(0)?,
                call_id: row.get(1)?,
                start_timestamp_ns: row.get(2)?,
                end_timestamp_ns: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                input_messages: row.get(8)?,
                output_messages: row.get(9)?,
                system_instructions: row.get(10)?,
                agent_name: row.get(11)?,
                process_name: row.get(12)?,
                pid: row.get(13)?,
                user_query: row.get(14)?,
                event_json: row.get(15)?,
                trace_id: row.get(16)?,
                cache_read_tokens: row.get(17)?,
                conversation_id: row.get(18)?,
                status: row.get(19)?,
                interruption_type: row.get(20)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Fetch all LLM call events for a given conversation ID (user query fingerprint).
    pub fn get_events_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<TraceEventDetail>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE conversation_id = ?1
               AND event_type = 'llm_call'
             ORDER BY start_timestamp_ns ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok(TraceEventDetail {
                id: row.get(0)?,
                call_id: row.get(1)?,
                start_timestamp_ns: row.get(2)?,
                end_timestamp_ns: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                input_messages: row.get(8)?,
                output_messages: row.get(9)?,
                system_instructions: row.get(10)?,
                agent_name: row.get(11)?,
                process_name: row.get(12)?,
                pid: row.get(13)?,
                user_query: row.get(14)?,
                event_json: row.get(15)?,
                trace_id: row.get(16)?,
                cache_read_tokens: row.get(17)?,
                conversation_id: row.get(18)?,
                status: row.get(19)?,
                interruption_type: row.get(20)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Fetch all LLM call events for a given session ID (across all traces).
    pub fn get_events_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<TraceEventDetail>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE session_id = ?1
               AND event_type = 'llm_call'
             ORDER BY start_timestamp_ns ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(TraceEventDetail {
                id: row.get(0)?,
                call_id: row.get(1)?,
                start_timestamp_ns: row.get(2)?,
                end_timestamp_ns: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                input_messages: row.get(8)?,
                output_messages: row.get(9)?,
                system_instructions: row.get(10)?,
                agent_name: row.get(11)?,
                process_name: row.get(12)?,
                pid: row.get(13)?,
                user_query: row.get(14)?,
                event_json: row.get(15)?,
                trace_id: row.get(16)?,
                cache_read_tokens: row.get(17)?,
                conversation_id: row.get(18)?,
                status: row.get(19)?,
                interruption_type: row.get(20)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Fetch all LLM call events within a timestamp range, optionally filtered by agent name.
    /// Used by skill_metrics module for on-demand metric computation.
    pub fn get_events_in_time_range(
        &self,
        start_ns: i64,
        end_ns: i64,
        agent_name: Option<&str>,
    ) -> Result<Vec<TraceEventDetail>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();

        let sql = if agent_name.is_some() {
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE start_timestamp_ns BETWEEN ?1 AND ?2
               AND event_type = 'llm_call'
               AND COALESCE(agent_name, process_name) = ?3
             ORDER BY start_timestamp_ns ASC"
        } else {
            "SELECT id, call_id, start_timestamp_ns, end_timestamp_ns,
                    model,
                    COALESCE(input_tokens, 0)  AS input_tokens,
                    COALESCE(output_tokens, 0) AS output_tokens,
                    COALESCE(total_tokens, 0)  AS total_tokens,
                    input_messages, output_messages, system_instructions,
                    agent_name, process_name, pid, user_query, event_json,
                    trace_id, cache_read_tokens, conversation_id, status, interruption_type
             FROM genai_events
             WHERE start_timestamp_ns BETWEEN ?1 AND ?2
               AND event_type = 'llm_call'
             ORDER BY start_timestamp_ns ASC"
        };

        let mut stmt = conn.prepare(sql)?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<TraceEventDetail> {
            Ok(TraceEventDetail {
                id: row.get(0)?,
                call_id: row.get(1)?,
                start_timestamp_ns: row.get(2)?,
                end_timestamp_ns: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                input_messages: row.get(8)?,
                output_messages: row.get(9)?,
                system_instructions: row.get(10)?,
                agent_name: row.get(11)?,
                process_name: row.get(12)?,
                pid: row.get(13)?,
                user_query: row.get(14)?,
                event_json: row.get(15)?,
                trace_id: row.get(16)?,
                cache_read_tokens: row.get(17)?,
                conversation_id: row.get(18)?,
                status: row.get(19)?,
                interruption_type: row.get(20)?,
            })
        };

        let mut result = Vec::new();
        if let Some(agent) = agent_name {
            let rows = stmt.query_map(params![start_ns, end_ns, agent], map_row)?;
            for row in rows {
                result.push(row?);
            }
        } else {
            let rows = stmt.query_map(params![start_ns, end_ns], map_row)?;
            for row in rows {
                result.push(row?);
            }
        }
        Ok(result)
    }

    /// Store a single GenAI event with size limit enforcement
    pub(super) fn store_event(
        &self,
        event: &GenAISemanticEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Check size before write and prune if needed
        self.check_and_prune_if_needed()?;

        // Attempt insert with retry on SQLITE_FULL
        let mut retries = 0;
        loop {
            match self.try_insert_event(event) {
                Ok(()) => {
                    return Ok(());
                }
                Err(e) => {
                    // Check if it's SQLITE_FULL (extended code 13)
                    if let Some(rusqlite::Error::SqliteFailure(err, _)) =
                        e.downcast_ref::<rusqlite::Error>()
                    {
                        if err.extended_code == 13 && retries < MAX_PRUNE_RETRIES {
                            retries += 1;
                            log::warn!(
                                "Database full (SQLITE_FULL), pruning old records (attempt {retries}/{MAX_PRUNE_RETRIES})"
                            );
                            self.prune_old_records()?;
                            self.checkpoint()?;
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Try to insert an event without size check
    fn try_insert_event(
        &self,
        event: &GenAISemanticEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let event_json = serde_json::to_string(event)?;

        match event {
            GenAISemanticEvent::LLMCall(call) => {
                let (input_tokens, output_tokens, total_tokens) = call
                    .token_usage
                    .as_ref()
                    .map(|u| {
                        (
                            u.input_tokens as i64,
                            u.output_tokens as i64,
                            u.total_tokens as i64,
                        )
                    })
                    .unwrap_or((0, 0, 0));
                let cache_creation = call
                    .token_usage
                    .as_ref()
                    .and_then(|u| u.cache_creation_input_tokens.map(|v| v as i64));
                let cache_read = call
                    .token_usage
                    .as_ref()
                    .and_then(|u| u.cache_read_input_tokens.map(|v| v as i64));

                // Extract system instructions
                let system_instructions: Option<String> = {
                    let sys_msgs: Vec<_> = call
                        .request
                        .messages
                        .iter()
                        .filter(|m| m.role == "system")
                        .collect();
                    if sys_msgs.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&sys_msgs).ok()
                    }
                };

                // Extract input messages (incremental: latest round only)
                let input_messages: Option<String> = {
                    let latest =
                        crate::genai::semantic::latest_round_input_messages(&call.request.messages);
                    if latest.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&latest).ok()
                    }
                };

                // Extract output messages
                let output_messages: Option<String> = if call.response.messages.is_empty() {
                    None
                } else {
                    serde_json::to_string(&call.response.messages).ok()
                };

                // Extract finish reasons
                let finish_reasons: Option<String> = if call.response.messages.is_empty() {
                    None
                } else {
                    let reasons: Vec<_> = call
                        .response
                        .messages
                        .iter()
                        .filter_map(|m| m.finish_reason.as_deref())
                        .collect();
                    if reasons.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&reasons).ok()
                    }
                };

                // Extract tool_call_ids from response messages (outgoing tool calls)
                let tool_call_ids: Option<String> = {
                    let ids: Vec<String> = call
                        .response
                        .messages
                        .iter()
                        .flat_map(|m| m.parts.iter())
                        .filter_map(|p| {
                            if let crate::genai::semantic::MessagePart::ToolCall {
                                id: Some(tc_id),
                                ..
                            } = p
                            {
                                Some(tc_id.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if ids.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&ids).ok()
                    }
                };

                // Get instance ID (same logic as SLS uploader)
                let instance = crate::genai::instance_id::get_instance_id();

                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, call_id, trace_id, conversation_id, session_id, instance,
                        start_timestamp_ns, end_timestamp_ns, duration_ns, pid, process_name, agent_name,
                        operation_name, provider, model, request_model, response_model,
                        temperature, max_tokens, top_p, frequency_penalty, presence_penalty,
                        finish_reasons, server_address,
                        input_tokens, output_tokens, total_tokens,
                        cache_creation_tokens, cache_read_tokens,
                        system_instructions, input_messages, output_messages,
                        user_query, http_method, http_path, status_code,
                        is_sse, sse_event_count, event_json, tool_call_ids
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                        ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32,
                        ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40
                    )",
                    params![
                        "llm_call",
                        call.call_id,
                        call.metadata.get("response_id"),
                        call.metadata.get("conversation_id"),
                        call.metadata.get("session_id"),
                        instance,
                        call.start_timestamp_ns as i64,
                        call.end_timestamp_ns as i64,
                        call.duration_ns as i64,
                        call.pid,
                        call.process_name,
                        call.agent_name,
                        call.metadata.get("operation_name"),
                        call.provider,
                        call.model,
                        call.model, // request_model
                        call.model, // response_model (same for now)
                        call.request.temperature,
                        call.request.max_tokens.map(|v| v as i64),
                        call.request.top_p,
                        call.request.frequency_penalty,
                        call.request.presence_penalty,
                        finish_reasons,
                        call.metadata.get("server.address"),
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        cache_creation,
                        cache_read,
                        system_instructions,
                        input_messages,
                        output_messages,
                        call.metadata.get("user_query"),
                        call.metadata.get("method"),
                        call.metadata.get("path"),
                        call.metadata.get("status_code").and_then(|s| s.parse::<i64>().ok()),
                        call.metadata.get("is_sse").map(|s| if s == "true" { 1i64 } else { 0 }),
                        call.metadata.get("sse_event_count").and_then(|s| s.parse::<i64>().ok()),
                        event_json,
                        tool_call_ids,
                    ],
                )?;
            }
            GenAISemanticEvent::ToolUse(tool) => {
                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, call_id, timestamp_ns, pid,
                        event_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        "tool_use",
                        tool.parent_llm_call_id,
                        tool.timestamp_ns as i64,
                        tool.pid,
                        event_json,
                    ],
                )?;
            }
            GenAISemanticEvent::AgentInteraction(interaction) => {
                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, timestamp_ns, pid,
                        event_json
                    ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        "agent_interaction",
                        interaction.timestamp_ns as i64,
                        interaction.pid,
                        event_json,
                    ],
                )?;
            }
            GenAISemanticEvent::StreamChunk(chunk) => {
                conn.execute(
                    "INSERT INTO genai_events (
                        event_type, call_id, timestamp_ns, pid,
                        event_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        "stream_chunk",
                        chunk.parent_llm_call_id,
                        chunk.timestamp_ns as i64,
                        chunk.pid,
                        event_json,
                    ],
                )?;
            }
        }
        Ok(())
    }
}

impl GenAIExporter for GenAISqliteStore {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn export(&self, events: &[GenAISemanticEvent]) {
        for event in events {
            if let Err(e) = self.store_event(event) {
                log::warn!("Failed to store GenAI event to SQLite: {e}");
            }
        }
    }
}
