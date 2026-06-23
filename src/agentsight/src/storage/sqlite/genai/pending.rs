//! Pending-call lifecycle methods for GenAI SQLite store.

use rusqlite::params;

use super::GenAISqliteStore;
use crate::genai::semantic::GenAISemanticEvent;

// ─── Query result types ────────────────────────────────────────────────────────

/// Lightweight info needed to write a PENDING record when a request is first seen
pub struct PendingCallInfo {
    /// Unique call ID (same one that will be used in the complete record)
    pub call_id: String,
    /// Trace ID (LLM API response_id, e.g. chatcmpl-xxx)
    pub trace_id: Option<String>,
    /// Conversation ID (user query fingerprint)
    pub conversation_id: Option<String>,
    /// Session ID
    pub session_id: Option<String>,
    /// Request start timestamp (nanoseconds)
    pub start_timestamp_ns: u64,
    /// Process ID
    pub pid: i32,
    /// Process name / comm
    pub process_name: String,
    /// Resolved agent name
    pub agent_name: Option<String>,
    /// HTTP method
    pub http_method: Option<String>,
    /// HTTP path
    pub http_path: Option<String>,
    /// Serialised input messages (JSON)
    pub input_messages: Option<String>,
    /// Serialised system instructions (JSON)
    pub system_instructions: Option<String>,
    /// User query extracted from request
    pub user_query: Option<String>,
    /// Whether this is an SSE streaming request
    pub is_sse: bool,
    /// Model name (extracted from request body)
    pub model: Option<String>,
    /// Provider name (extracted from request path)
    pub provider: Option<String>,
}

/// Data extracted from captured SSE events for enriching a pending record.
pub struct SseEnrichment {
    pub model: Option<String>,
    pub trace_id: Option<String>,
    pub provider: Option<String>,
    pub output_messages: Option<String>,
    pub sse_event_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

impl GenAISqliteStore {
    // ─── Pending-call lifecycle methods ────────────────────────────────────────

    /// Insert a PENDING record as soon as a request is captured.
    ///
    /// The record is later promoted to 'complete' via [`complete_pending`] once
    /// the full response arrives, or marked 'interrupted' by the stale-scan thread
    /// if the agent crashes before the response is received.
    pub fn insert_pending(&self, info: &PendingCallInfo) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let instance = crate::genai::instance_id::get_instance_id();
        conn.execute(
            "INSERT INTO genai_events (
                event_type, status, call_id, trace_id, conversation_id, session_id, instance,
                start_timestamp_ns, pid, process_name, agent_name,
                http_method, http_path, is_sse,
                input_messages, system_instructions, user_query,
                model, provider,
                event_json
            ) VALUES (
                'llm_call', 'pending', ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9,
                ?10, ?11, ?12,
                ?13, ?14, ?15,
                ?16, ?17,
                '{}'
            )",
            params![
                info.call_id,
                info.trace_id,
                info.conversation_id,
                info.session_id,
                instance,
                info.start_timestamp_ns as i64,
                info.pid,
                info.process_name,
                info.agent_name,
                info.http_method,
                info.http_path,
                if info.is_sse { 1i64 } else { 0 },
                info.input_messages,
                info.system_instructions,
                info.user_query,
                info.model,
                info.provider,
            ],
        )?;
        Ok(())
    }

    /// Promote an existing PENDING record to 'complete' by updating all response fields.
    ///
    /// If no matching PENDING row exists (e.g. because the DB was restarted), the
    /// call falls back to a plain INSERT so data is never silently dropped.
    pub fn complete_pending(
        &self,
        event: &GenAISemanticEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let GenAISemanticEvent::LLMCall(call) = event {
            {
                let conn = self.conn.lock().unwrap();
                let event_json = serde_json::to_string(event)?;

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

                let system_instructions: Option<String> = {
                    let sys: Vec<_> = call
                        .request
                        .messages
                        .iter()
                        .filter(|m| m.role == "system")
                        .collect();
                    if sys.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&sys).ok()
                    }
                };
                let input_messages: Option<String> = {
                    let latest =
                        crate::genai::semantic::latest_round_input_messages(&call.request.messages);
                    if latest.is_empty() {
                        None
                    } else {
                        serde_json::to_string(&latest).ok()
                    }
                };
                let output_messages: Option<String> = if call.response.messages.is_empty() {
                    None
                } else {
                    serde_json::to_string(&call.response.messages).ok()
                };
                let finish_reasons: Option<String> = {
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

                let updated = conn.execute(
                    "UPDATE genai_events SET
                        status = 'complete',
                        trace_id            = ?1,
                        conversation_id     = ?2,
                        session_id          = ?3,
                        end_timestamp_ns    = ?4,
                        duration_ns         = ?5,
                        provider            = ?6,
                        model               = ?7,
                        request_model       = ?8,
                        response_model      = ?9,
                        temperature         = ?10,
                        max_tokens          = ?11,
                        top_p               = ?12,
                        frequency_penalty   = ?13,
                        presence_penalty    = ?14,
                        finish_reasons      = ?15,
                        server_address      = ?16,
                        input_tokens        = ?17,
                        output_tokens       = ?18,
                        total_tokens        = ?19,
                        cache_creation_tokens = ?20,
                        cache_read_tokens   = ?21,
                        system_instructions = ?22,
                        input_messages      = ?23,
                        output_messages     = ?24,
                        status_code         = ?25,
                        sse_event_count     = ?26,
                        event_json          = ?27,
                        tool_call_ids       = ?28
                    WHERE call_id = ?29 AND status IN ('pending', 'interrupted')",
                    params![
                        call.metadata.get("response_id"),
                        call.metadata.get("conversation_id"),
                        call.metadata.get("session_id"),
                        call.end_timestamp_ns as i64,
                        call.duration_ns as i64,
                        call.provider,
                        call.model,
                        call.model,
                        call.model,
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
                        call.metadata
                            .get("status_code")
                            .and_then(|s| s.parse::<i64>().ok()),
                        call.metadata
                            .get("sse_event_count")
                            .and_then(|s| s.parse::<i64>().ok()),
                        event_json,
                        tool_call_ids,
                        call.call_id,
                    ],
                )?;

                if updated > 0 {
                    log::debug!(
                        "[GenAI] Promoted pending/interrupted→complete for call_id={}",
                        call.call_id
                    );
                    return Ok(());
                }
                // Row exists with status='complete' — already done, skip.
                let exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM genai_events WHERE call_id = ?1)",
                    params![call.call_id],
                    |row| row.get(0),
                )?;
                if exists {
                    log::debug!(
                        "[GenAI] Row already exists for call_id={} (complete), skipping",
                        call.call_id
                    );
                    return Ok(());
                }
                log::debug!(
                    "[GenAI] No row for call_id={}, inserting directly",
                    call.call_id
                );
            }
            // Fallback: store_event handles the full INSERT path
            self.store_event(event)
        } else {
            // Non-LLMCall events have no pending lifecycle, store directly
            self.store_event(event)
        }
    }

    /// Mark stale PENDING records as 'interrupted'.
    ///
    /// Called by the background scanner.  Any `llm_call` row that has been in
    /// 'pending' state for longer than `timeout_secs` is assumed to have been
    /// lost (agent crash / network cut) and is updated to 'interrupted'.
    ///
    /// Returns the number of rows updated.
    pub fn mark_interrupted_stale(
        &self,
        timeout_secs: u64,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let cutoff_ns = {
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            now_ns - (timeout_secs as i64 * 1_000_000_000)
        };
        let updated = conn.execute(
            "UPDATE genai_events
             SET status = 'interrupted'
             WHERE event_type = 'llm_call'
               AND status = 'pending'
               AND start_timestamp_ns < ?1",
            params![cutoff_ns],
        )?;
        if updated > 0 {
            log::info!("[GenAI] Marked {updated} stale pending call(s) as interrupted");
        }
        Ok(updated)
    }

    /// Set the interruption_type for a specific call_id.
    ///
    /// Called by the online InterruptionDetector after detecting an anomaly.
    pub fn update_interruption_type(
        &self,
        call_id: &str,
        itype: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE genai_events SET interruption_type = ?1 WHERE call_id = ?2",
            params![itype, call_id],
        )?;
        Ok(())
    }

    pub fn count_interruption_type_for_conversation(
        &self,
        conversation_id: &str,
        itype: &str,
    ) -> u32 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM genai_events WHERE conversation_id = ?1 AND interruption_type = ?2",
            params![conversation_id, itype],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    /// Fetch the most recent N LLM calls for a conversation (for loop detection).
    ///
    /// Returns lightweight summaries ordered oldest-first (ascending timestamp).
    /// Used by LoopDetector to analyze repetitive patterns across calls.
    pub fn get_recent_calls_for_conversation(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Vec<crate::interruption::RecentCallSummary> {
        let conn = self.conn.lock().unwrap();
        // Subquery fetches latest N rows desc, outer query reverses to asc order
        let sql = "SELECT call_id, output_messages, COALESCE(input_tokens, 0), COALESCE(output_tokens, 0) \
                   FROM (SELECT call_id, output_messages, input_tokens, output_tokens, start_timestamp_ns \
                         FROM genai_events \
                         WHERE event_type = 'llm_call' \
                           AND conversation_id = ?1 \
                           AND status != 'pending' \
                         ORDER BY start_timestamp_ns DESC \
                         LIMIT ?2) \
                   ORDER BY start_timestamp_ns ASC";
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = match stmt.query_map(params![conversation_id, limit as i64], |row| {
            let call_id: String = row.get(0)?;
            let output_messages_json: Option<String> = row.get(1)?;
            let input_tokens: i64 = row.get(2)?;
            let output_tokens: i64 = row.get(3)?;
            Ok((call_id, output_messages_json, input_tokens, output_tokens))
        }) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        rows.filter_map(|r| r.ok())
            .map(|(call_id, output_json, input_tokens, output_tokens)| {
                let (tool_call_names, output_text_snippet) =
                    parse_output_messages_for_loop_detection(output_json.as_deref());
                crate::interruption::RecentCallSummary {
                    call_id,
                    tool_call_names,
                    output_text_snippet,
                    input_tokens,
                    output_tokens,
                }
            })
            .collect()
    }

    /// List all pending calls for a specific PID.
    ///
    /// Returns (call_id, session_id, trace_id, conversation_id) tuples for all
    /// PENDING records matching the given PID. Used by HealthChecker to link
    /// agent_crash events to their associated LLM calls.
    pub fn list_pending_for_pid(
        &self,
        pid: i32,
    ) -> Result<
        Vec<(String, Option<String>, Option<String>, Option<String>)>,
        Box<dyn std::error::Error>,
    > {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT call_id, session_id, trace_id, conversation_id
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND status = 'pending'
               AND pid = ?1",
        )?;
        let rows = stmt
            .query_map(params![pid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Mark all pending calls for a PID as interrupted.
    ///
    /// Called by HealthChecker when it detects an agent process has gone offline.
    /// Sets status='interrupted' and interruption_type to the provided value.
    pub fn mark_pending_interrupted_for_pid(
        &self,
        pid: i32,
        itype: &str,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE genai_events
             SET status = 'interrupted', interruption_type = ?1
             WHERE event_type = 'llm_call'
               AND status = 'pending'
               AND pid = ?2",
            params![itype, pid],
        )?;
        if updated > 0 {
            log::info!("Marked {updated} pending call(s) as interrupted for pid={pid}");
        }
        Ok(updated)
    }

    /// Like `list_pending_for_pid` but accepts multiple PIDs at once.
    pub fn list_pending_for_pids(
        &self,
        pids: &[i32],
    ) -> Result<
        Vec<(String, Option<String>, Option<String>, Option<String>)>,
        Box<dyn std::error::Error>,
    > {
        if pids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock().unwrap();
        let placeholders: String = pids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT call_id, session_id, trace_id, conversation_id
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND status = 'pending'
               AND pid IN ({placeholders})"
        );
        let params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = pids
            .iter()
            .map(|p| Box::new(*p) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Enrich a pending record with data extracted from captured SSE events.
    /// Updates model, trace_id, provider, output_messages, sse_event_count, and token counts.
    pub fn enrich_pending_from_sse(
        &self,
        call_id: &str,
        enrichment: &SseEnrichment,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE genai_events SET
                model            = COALESCE(?2, model),
                trace_id         = COALESCE(?3, trace_id),
                provider         = COALESCE(?4, provider),
                output_messages  = COALESCE(?5, output_messages),
                sse_event_count  = COALESCE(?6, sse_event_count),
                input_tokens     = COALESCE(?7, input_tokens),
                output_tokens    = COALESCE(?8, output_tokens),
                total_tokens     = COALESCE(?7, input_tokens, 0)
                                 + COALESCE(?8, output_tokens, 0)
             WHERE call_id = ?1",
            params![
                call_id,
                enrichment.model,
                enrichment.trace_id,
                enrichment.provider,
                enrichment.output_messages,
                enrichment.sse_event_count,
                enrichment.input_tokens,
                enrichment.output_tokens,
            ],
        )?;
        Ok(())
    }
}

// ─── Helper for loop detection ───────────────────────────────────────────────

/// Parse the `output_messages` JSON column to extract tool call names and text snippets.
///
/// The JSON structure follows the OTel GenAI parts format stored by `store_event()`:
/// ```json
/// [{"role":"assistant","parts":[{"type":"tool_call","name":"read_file",...},{"type":"text","content":"..."}]}]
/// ```
pub(super) fn parse_output_messages_for_loop_detection(
    json_str: Option<&str>,
) -> (Vec<String>, String) {
    let Some(json_str) = json_str else {
        return (vec![], String::new());
    };

    let messages: Vec<serde_json::Value> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return (vec![], String::new()),
    };

    let mut tool_names = Vec::new();
    let mut text_parts = Vec::new();

    for msg in &messages {
        if let Some(parts) = msg.get("parts").and_then(|p| p.as_array()) {
            for part in parts {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("tool_call") => {
                        if let Some(name) = part.get("name").and_then(|n| n.as_str()) {
                            tool_names.push(name.to_string());
                        }
                    }
                    Some("text") => {
                        if let Some(content) = part.get("content").and_then(|c| c.as_str()) {
                            text_parts.push(content);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Build a snippet from text parts (max 200 chars)
    let full_text = text_parts.join(" ");
    let snippet = if full_text.len() > 200 {
        full_text.chars().take(200).collect()
    } else {
        full_text
    };

    (tool_names, snippet)
}
