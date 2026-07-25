use super::*;
use crate::genai::semantic::{
    GenAISemanticEvent, LLMCall, LLMRequest, LLMResponse, MessagePart, OutputMessage,
};

/// Integration test: store_event (post-fix, no per-insert VACUUM) still
/// persists data correctly and the row is immediately readable.
/// Reverting the VACUUM removal does NOT make this test fail (it would just
/// be slower), but this proves the write path is functional — the
/// discriminating signal for the per-insert VACUUM removal is the latency
/// benchmark, not a correctness test.
#[test]
fn store_event_persists_without_per_insert_vacuum() {
    let path = std::env::temp_dir().join(format!(
        "test_genai_store_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = GenAISqliteStore::new_with_path(&path).unwrap();

    let call = LLMCall::new(
        "test-call-001".to_string(),
        1_700_000_000_000_000_000,
        "openai".to_string(),
        "gpt-4".to_string(),
        LLMRequest {
            messages: vec![],
            temperature: None,
            max_tokens: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: None,
        },
        1234,
        "test-agent".to_string(),
    );
    let event = GenAISemanticEvent::LLMCall(call);

    // Write via the exact code path that was modified (store_event).
    store.store_event(&event).unwrap();

    // The event has no session_id set, so list_sessions (which filters
    // session_id IS NOT NULL) won't find it — use a raw count instead.
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM genai_events WHERE call_id = 'test-call-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "store_event must persist the row");

    drop(conn);
    // Verify wal_checkpoint doesn't panic
    store.wal_checkpoint().unwrap();

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

/// Verify busy_timeout is set on connections (create_connection is used by
/// GenAISqliteStore::new_with_path internally).
#[test]
fn connection_has_busy_timeout() {
    let path = std::env::temp_dir().join(format!(
        "test_bt_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = GenAISqliteStore::new_with_path(&path).unwrap();
    let conn = store.conn.lock().unwrap();
    // PRAGMA busy_timeout returns the current value in ms
    let timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    assert_eq!(timeout, 500, "busy_timeout must be 500ms");
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

use super::pending::parse_output_messages_for_loop_detection;

#[test]
fn test_parse_output_none() {
    let (tools, text) = parse_output_messages_for_loop_detection(None);
    assert!(tools.is_empty());
    assert!(text.is_empty());
}

#[test]
fn test_parse_output_invalid_json() {
    let (tools, text) = parse_output_messages_for_loop_detection(Some("not json"));
    assert!(tools.is_empty());
    assert!(text.is_empty());
}

#[test]
fn test_parse_output_tool_calls_only() {
    let json = r#"[{"role":"assistant","parts":[{"type":"tool_call","name":"read_file"},{"type":"tool_call","name":"write_file"}]}]"#;
    let (tools, text) = parse_output_messages_for_loop_detection(Some(json));
    assert_eq!(tools, vec!["read_file", "write_file"]);
    assert!(text.is_empty());
}

#[test]
fn test_parse_output_text_only() {
    let json = r#"[{"role":"assistant","parts":[{"type":"text","content":"Hello world"}]}]"#;
    let (tools, text) = parse_output_messages_for_loop_detection(Some(json));
    assert!(tools.is_empty());
    assert_eq!(text, "Hello world");
}

#[test]
fn test_parse_output_mixed() {
    let json = r#"[{"role":"assistant","parts":[{"type":"tool_call","name":"search"},{"type":"text","content":"Found results"}]}]"#;
    let (tools, text) = parse_output_messages_for_loop_detection(Some(json));
    assert_eq!(tools, vec!["search"]);
    assert_eq!(text, "Found results");
}

#[test]
fn test_parse_output_multiple_text_parts() {
    let json = r#"[{"role":"assistant","parts":[{"type":"text","content":"Part 1"},{"type":"text","content":"Part 2"}]}]"#;
    let (_tools, text) = parse_output_messages_for_loop_detection(Some(json));
    assert_eq!(text, "Part 1 Part 2");
}

#[test]
fn test_parse_output_text_truncated_at_200_chars() {
    let long_content = "a".repeat(300);
    let json = format!(
        r#"[{{"role":"assistant","parts":[{{"type":"text","content":"{long_content}"}}]}}]"#
    );
    let (_, text) = parse_output_messages_for_loop_detection(Some(&json));
    assert_eq!(text.len(), 200);
}

#[test]
fn test_parse_output_empty_parts_array() {
    let json = r#"[{"role":"assistant","parts":[]}]"#;
    let (tools, text) = parse_output_messages_for_loop_detection(Some(json));
    assert!(tools.is_empty());
    assert!(text.is_empty());
}

#[test]
fn test_parse_output_no_parts_field() {
    let json = r#"[{"role":"assistant"}]"#;
    let (tools, text) = parse_output_messages_for_loop_detection(Some(json));
    assert!(tools.is_empty());
    assert!(text.is_empty());
}

// ─── Populated test store helpers ─────────────────────────────────────────────

use rusqlite::params;

const BASE_NS: i64 = 1_700_000_000_000_000_000;
const STEP_NS: i64 = 1_000_000_000;

fn cleanup_db(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

/// Create a store with 6 representative rows covering multiple sessions,
/// agents, models, tool_call_ids, and a pending record.
///
/// Layout (all event_type = 'llm_call'):
///   call-1: sess-1, agent-a, gpt-4,   trace-1, conv-1, pid=100, complete, tool_call_ids
///   call-2: sess-1, agent-a, gpt-4,   trace-1, conv-1, pid=100, complete
///   call-3: sess-1, agent-a, gpt-4,   trace-2, conv-2, pid=100, complete, user_query
///   call-4: sess-2, agent-b, claude-3, trace-3, conv-3, pid=200, complete
///   call-5: sess-2, agent-b, claude-3, trace-3, conv-3, pid=200, pending
///   call-6: sess-1, agent-a, gpt-4o,  trace-1, conv-1, pid=100, complete
fn create_populated_store(suffix: &str) -> (GenAISqliteStore, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("test_genai_{suffix}_{}.db", std::process::id()));
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();

    let b = BASE_NS;
    let s = STEP_NS;
    let sql = "INSERT INTO genai_events (\
               call_id, event_type, start_timestamp_ns, end_timestamp_ns, duration_ns,\
               provider, model, input_tokens, output_tokens, total_tokens,\
               session_id, trace_id, conversation_id, agent_name, pid,\
               status, tool_call_ids, event_json, process_name, user_query\
               ) VALUES (?1,'llm_call',?2,?3,?4,?5,?6,?7,?8,?9,\
               ?10,?11,?12,?13,?14,?15,?16,'{}',?17,?18)";

    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            sql,
            params![
                "call-1",
                b,
                b + s,
                s,
                "openai",
                "gpt-4",
                100_i64,
                50_i64,
                150_i64,
                "sess-1",
                "trace-1",
                "conv-1",
                "agent-a",
                100_i32,
                "complete",
                r#"["tc-1","tc-2"]"#,
                "proc-a",
                None::<&str>
            ],
        )
        .unwrap();
        conn.execute(
            sql,
            params![
                "call-2",
                b + s,
                b + 2 * s,
                s,
                "openai",
                "gpt-4",
                200_i64,
                100_i64,
                300_i64,
                "sess-1",
                "trace-1",
                "conv-1",
                "agent-a",
                100_i32,
                "complete",
                None::<&str>,
                "proc-a",
                None::<&str>
            ],
        )
        .unwrap();
        conn.execute(
            sql,
            params![
                "call-3",
                b + 2 * s,
                b + 3 * s,
                s,
                "openai",
                "gpt-4",
                150_i64,
                75_i64,
                225_i64,
                "sess-1",
                "trace-2",
                "conv-2",
                "agent-a",
                100_i32,
                "complete",
                None::<&str>,
                "proc-a",
                "what is rust"
            ],
        )
        .unwrap();
        conn.execute(
            sql,
            params![
                "call-4",
                b + 3 * s,
                b + 4 * s,
                s,
                "anthropic",
                "claude-3",
                300_i64,
                150_i64,
                450_i64,
                "sess-2",
                "trace-3",
                "conv-3",
                "agent-b",
                200_i32,
                "complete",
                None::<&str>,
                "proc-b",
                None::<&str>
            ],
        )
        .unwrap();
        conn.execute(
            sql,
            params![
                "call-5",
                b + 4 * s,
                b + 5 * s,
                s,
                "anthropic",
                "claude-3",
                250_i64,
                125_i64,
                375_i64,
                "sess-2",
                "trace-3",
                "conv-3",
                "agent-b",
                200_i32,
                "pending",
                None::<&str>,
                "proc-b",
                None::<&str>
            ],
        )
        .unwrap();
        conn.execute(
            sql,
            params![
                "call-6",
                b + 5 * s,
                b + 6 * s,
                s,
                "openai",
                "gpt-4o",
                50_i64,
                25_i64,
                75_i64,
                "sess-1",
                "trace-1",
                "conv-1",
                "agent-a",
                100_i32,
                "complete",
                None::<&str>,
                "proc-a",
                None::<&str>
            ],
        )
        .unwrap();
    }
    (store, path)
}

// ─── stats.rs tests ───────────────────────────────────────────────────────────

#[test]
fn test_get_token_timeseries_returns_buckets() {
    let (store, path) = create_populated_store("ts_buckets");
    let r = store
        .get_token_timeseries(BASE_NS, BASE_NS + 6 * STEP_NS, None, 1)
        .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].input_tokens, 1050); // 100+200+150+300+250+50
    assert_eq!(r[0].output_tokens, 525);
    assert_eq!(r[0].total_tokens, 1575);
    cleanup_db(&path);
}

#[test]
fn test_get_token_timeseries_empty_range() {
    let (store, path) = create_populated_store("ts_empty");
    let r = store.get_token_timeseries(0, 1, None, 1).unwrap();
    assert!(r.is_empty());
    cleanup_db(&path);
}

#[test]
fn test_get_token_timeseries_with_agent_filter() {
    let (store, path) = create_populated_store("ts_agent");
    let r = store
        .get_token_timeseries(BASE_NS, BASE_NS + 6 * STEP_NS, Some("agent-a"), 1)
        .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].total_tokens, 750); // 150+300+225+75
    cleanup_db(&path);
}

#[test]
fn test_get_model_timeseries_returns_model_breakdown() {
    let (store, path) = create_populated_store("model_ts");
    let r = store
        .get_model_timeseries(BASE_NS, BASE_NS + 6 * STEP_NS, None, 1)
        .unwrap();
    assert_eq!(r.len(), 3);
    let gpt4 = r.iter().find(|b| b.model == "gpt-4").unwrap();
    assert_eq!(gpt4.total_tokens, 675); // 150+300+225
    let claude = r.iter().find(|b| b.model == "claude-3").unwrap();
    assert_eq!(claude.total_tokens, 825); // 450+375
    cleanup_db(&path);
}

#[test]
fn test_get_model_timeseries_with_agent_filter() {
    let (store, path) = create_populated_store("model_ts_agent");
    let r = store
        .get_model_timeseries(BASE_NS, BASE_NS + 6 * STEP_NS, Some("agent-a"), 1)
        .unwrap();
    assert_eq!(r.len(), 2); // gpt-4, gpt-4o
    cleanup_db(&path);
}

#[test]
fn test_get_agent_token_summary() {
    let (store, path) = create_populated_store("agent_summary");
    let r = store.get_agent_token_summary().unwrap();
    assert_eq!(r.len(), 2);
    // ORDER BY total_tokens DESC
    assert_eq!(r[0].agent_name, "agent-b");
    assert_eq!(r[0].total_tokens, 825);
    assert_eq!(r[0].request_count, 2);
    assert_eq!(r[1].agent_name, "agent-a");
    assert_eq!(r[1].total_tokens, 750);
    assert_eq!(r[1].request_count, 4);
    cleanup_db(&path);
}

#[test]
fn test_get_agent_token_summary_empty() {
    let path = std::env::temp_dir().join(format!("test_genai_ats_empty_{}.db", std::process::id()));
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();
    assert!(store.get_agent_token_summary().unwrap().is_empty());
    cleanup_db(&path);
}

// ─── session.rs tests ─────────────────────────────────────────────────────────

#[test]
fn test_list_sessions() {
    let (store, path) = create_populated_store("list_sess");
    let r = store
        .list_sessions(BASE_NS, BASE_NS + 6 * STEP_NS, true)
        .unwrap();
    assert_eq!(r.len(), 2);
    // sess-1 last_seen=base+5s > sess-2 base+4s
    assert_eq!(r[0].session_id, "sess-1");
    assert_eq!(r[0].conversation_count, 2);
    assert_eq!(r[0].total_input_tokens, 500);
    // Only call-3 in sess-1 carries a user_query, so it is both preview ends.
    assert_eq!(r[0].first_user_query.as_deref(), Some("what is rust"));
    assert_eq!(r[0].last_user_query.as_deref(), Some("what is rust"));
    assert_eq!(r[1].session_id, "sess-2");
    assert_eq!(r[1].total_input_tokens, 550);
    assert_eq!(r[1].first_user_query, None);
    assert_eq!(r[1].last_user_query, None);
    cleanup_db(&path);
}

#[test]
fn test_list_sessions_for_savings() {
    let (store, path) = create_populated_store("savings_no_agent");
    let r = store
        .list_sessions_for_savings(BASE_NS, BASE_NS + 6 * STEP_NS, None)
        .unwrap();
    assert_eq!(r.len(), 2);
    cleanup_db(&path);
}

#[test]
fn test_list_sessions_for_savings_with_agent_filter() {
    let (store, path) = create_populated_store("savings_agent");
    let r = store
        .list_sessions_for_savings(BASE_NS, BASE_NS + 6 * STEP_NS, Some("agent-b"))
        .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].session_id, "sess-2");
    assert_eq!(r[0].request_count, 2);
    cleanup_db(&path);
}

#[test]
fn test_get_session_for_savings() {
    let (store, path) = create_populated_store("get_savings");
    let s = store.get_session_for_savings("sess-1").unwrap().unwrap();
    assert_eq!(s.session_id, "sess-1");
    assert_eq!(s.total_input_tokens, 500);
    assert_eq!(s.total_output_tokens, 250);
    assert_eq!(s.request_count, 4);
    cleanup_db(&path);
}

#[test]
fn test_get_session_for_savings_not_found() {
    let (store, path) = create_populated_store("get_savings_404");
    assert!(
        store
            .get_session_for_savings("nonexistent")
            .unwrap()
            .is_none()
    );
    cleanup_db(&path);
}

#[test]
fn test_get_call_turn_indices() {
    let (store, path) = create_populated_store("call_turns");
    let m = store.get_call_turn_indices(&["sess-1"]).unwrap();
    assert_eq!(m.len(), 4);
    assert_eq!(m["call-1"], 1);
    assert_eq!(m["call-2"], 2);
    assert_eq!(m["call-3"], 3);
    assert_eq!(m["call-6"], 4);
    cleanup_db(&path);
}

#[test]
fn test_get_tool_call_turn_indices() {
    let (store, path) = create_populated_store("tc_turns");
    let m = store.get_tool_call_turn_indices(&["sess-1"]).unwrap();
    assert_eq!(m["tc-1"].turn_index, 1);
    assert_eq!(m["tc-1"].session_id, "sess-1");
    assert_eq!(m["tc-2"].turn_index, 1);
    assert!(m.contains_key("call-1"));
    cleanup_db(&path);
}

#[test]
fn test_list_traces_by_session() {
    let (store, path) = create_populated_store("traces");
    let r = store
        .list_traces_by_session("sess-1", None, None, true)
        .unwrap();
    assert_eq!(r.len(), 2);
    let c1 = r.iter().find(|t| t.conversation_id == "conv-1").unwrap();
    assert_eq!(c1.call_count, 3);
    assert_eq!(c1.total_input_tokens, 350); // 100+200+50
    let c2 = r.iter().find(|t| t.conversation_id == "conv-2").unwrap();
    assert_eq!(c2.call_count, 1);
    assert_eq!(c2.user_query.as_deref(), Some("what is rust"));
    cleanup_db(&path);
}

#[test]
fn test_list_traces_by_session_with_time_range() {
    let (store, path) = create_populated_store("traces_range");
    let r = store
        .list_traces_by_session("sess-1", Some(BASE_NS), Some(BASE_NS + STEP_NS), true)
        .unwrap();
    assert_eq!(r.len(), 1); // only conv-1
    assert_eq!(r[0].call_count, 2); // call-1, call-2
    cleanup_db(&path);
}

#[test]
fn test_list_agent_names() {
    let (store, path) = create_populated_store("agent_names");
    let r = store
        .list_agent_names(BASE_NS, BASE_NS + 6 * STEP_NS)
        .unwrap();
    assert_eq!(r, vec!["agent-a", "agent-b"]);
    cleanup_db(&path);
}

#[test]
fn test_lookup_session_for_pid() {
    let (store, path) = create_populated_store("lookup_pid");
    assert_eq!(
        store.lookup_session_for_pid(100).unwrap().as_deref(),
        Some("sess-1")
    );
    assert!(store.lookup_session_for_pid(999).unwrap().is_none());
    cleanup_db(&path);
}

#[test]
fn test_update_session_id() {
    let (store, path) = create_populated_store("update_sess");
    store.update_session_id("call-1", "sess-new").unwrap();
    let conn = store.conn.lock().unwrap();
    let sid: String = conn
        .query_row(
            "SELECT session_id FROM genai_events WHERE call_id = 'call-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sid, "sess-new");
    drop(conn);
    cleanup_db(&path);
}

// ─── events.rs tests ──────────────────────────────────────────────────────────

#[test]
fn test_get_trace_events() {
    let (store, path) = create_populated_store("trace_events");
    let r = store.get_trace_events("trace-1").unwrap();
    assert_eq!(r.len(), 3); // call-1, call-2, call-6
    assert_eq!(r[0].call_id.as_deref(), Some("call-1"));
    assert_eq!(r[0].input_tokens, 100);
    assert_eq!(r[2].call_id.as_deref(), Some("call-6"));
    cleanup_db(&path);
}

#[test]
fn test_get_events_by_conversation() {
    let (store, path) = create_populated_store("conv_events");
    let r = store.get_events_by_conversation("conv-3").unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].call_id.as_deref(), Some("call-4"));
    assert_eq!(r[1].call_id.as_deref(), Some("call-5"));
    cleanup_db(&path);
}

#[test]
fn test_get_events_by_session() {
    let (store, path) = create_populated_store("sess_events");
    let r = store.get_events_by_session("sess-2").unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].model.as_deref(), Some("claude-3"));
    cleanup_db(&path);
}

#[test]
fn test_get_events_in_time_range() {
    let (store, path) = create_populated_store("range_events");
    let r = store
        .get_events_in_time_range(BASE_NS + 2 * STEP_NS, BASE_NS + 3 * STEP_NS, None)
        .unwrap();
    assert_eq!(r.len(), 2); // call-3, call-4
    cleanup_db(&path);
}

#[test]
fn test_get_events_in_time_range_with_agent_filter() {
    let (store, path) = create_populated_store("range_agent");
    let r = store
        .get_events_in_time_range(BASE_NS, BASE_NS + 6 * STEP_NS, Some("agent-b"))
        .unwrap();
    assert_eq!(r.len(), 2); // call-4, call-5
    cleanup_db(&path);
}

// ─── pending.rs tests ─────────────────────────────────────────────────────────

#[test]
fn test_insert_pending() {
    let path = std::env::temp_dir().join(format!("test_genai_ins_pend_{}.db", std::process::id()));
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();
    let info = PendingCallInfo {
        call_id: "p-001".to_string(),
        trace_id: Some("t-p".to_string()),
        conversation_id: Some("c-p".to_string()),
        session_id: Some("s-p".to_string()),
        start_timestamp_ns: BASE_NS as u64,
        pid: 42,
        process_name: "test-proc".to_string(),
        agent_name: Some("test-agent".to_string()),
        http_method: Some("POST".to_string()),
        http_path: Some("/v1/chat".to_string()),
        input_messages: None,
        system_instructions: None,
        user_query: Some("hello".to_string()),
        is_sse: true,
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        call_kind: "main".to_string(),
        pending_origin: PendingOrigin::RequestCapture,
        pending_match_key: None,
        process_type: None,
    };
    store.insert_pending(&info).unwrap();
    let conn = store.conn.lock().unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM genai_events WHERE call_id = 'p-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "pending");
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_insert_pending_records_idle_origin_and_match_key() {
    let path =
        std::env::temp_dir().join(format!("test_genai_idle_origin_{}.db", std::process::id()));
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();
    let info = PendingCallInfo {
        call_id: "idle-temp".to_string(),
        trace_id: None,
        conversation_id: Some("c-idle".to_string()),
        session_id: Some("s-idle".to_string()),
        start_timestamp_ns: BASE_NS as u64,
        pid: 42,
        process_name: "claude".to_string(),
        agent_name: Some("claude".to_string()),
        http_method: Some("POST".to_string()),
        http_path: Some("/v1/messages".to_string()),
        input_messages: None,
        system_instructions: None,
        user_query: Some("hello".to_string()),
        is_sse: true,
        model: Some("claude-sonnet".to_string()),
        provider: Some("anthropic".to_string()),
        call_kind: "main".to_string(),
        pending_origin: PendingOrigin::IdleDrain,
        pending_match_key: Some("match-idle-1".to_string()),
        process_type: None,
    };
    store.insert_pending(&info).unwrap();

    let conn = store.conn.lock().unwrap();
    let (origin, match_key): (String, String) = conn
        .query_row(
            "SELECT pending_origin, pending_match_key FROM genai_events WHERE call_id = 'idle-temp'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(origin, "idle_drain");
    assert_eq!(match_key, "match-idle-1");
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_complete_pending_promotes_idle_snapshot_by_match_key() {
    let path =
        std::env::temp_dir().join(format!("test_genai_idle_promote_{}.db", std::process::id()));
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();
    let info = PendingCallInfo {
        call_id: "idle-temp".to_string(),
        trace_id: None,
        conversation_id: Some("c-idle".to_string()),
        session_id: Some("s-idle".to_string()),
        start_timestamp_ns: BASE_NS as u64,
        pid: 42,
        process_name: "claude".to_string(),
        agent_name: Some("claude".to_string()),
        http_method: Some("POST".to_string()),
        http_path: Some("/v1/messages".to_string()),
        input_messages: None,
        system_instructions: None,
        user_query: Some("hello".to_string()),
        is_sse: true,
        model: Some("claude-sonnet".to_string()),
        provider: Some("anthropic".to_string()),
        call_kind: "main".to_string(),
        pending_origin: PendingOrigin::IdleDrain,
        pending_match_key: Some("match-idle-2".to_string()),
        process_type: None,
    };
    store.insert_pending(&info).unwrap();

    let request = LLMRequest {
        messages: vec![],
        temperature: None,
        max_tokens: None,
        frequency_penalty: None,
        presence_penalty: None,
        top_p: None,
        top_k: None,
        seed: None,
        stop_sequences: None,
        stream: true,
        tools: None,
        raw_body: None,
    };
    let mut call = LLMCall::new(
        "real-response-id".to_string(),
        BASE_NS as u64,
        "anthropic".to_string(),
        "claude-sonnet".to_string(),
        request,
        42,
        "claude".to_string(),
    );
    call.set_response(
        LLMResponse {
            messages: vec![OutputMessage {
                role: "assistant".to_string(),
                parts: vec![MessagePart::Text {
                    content: "done".to_string(),
                }],
                name: None,
                finish_reason: Some("stop".to_string()),
            }],
            streamed: true,
            raw_body: None,
        },
        (BASE_NS + STEP_NS) as u64,
    );
    call.metadata
        .insert("response_id".to_string(), "real-response-id".to_string());
    call.metadata
        .insert("pending_match_key".to_string(), "match-idle-2".to_string());
    call.metadata
        .insert("conversation_id".to_string(), "c-real".to_string());
    call.metadata
        .insert("session_id".to_string(), "s-real".to_string());
    call.metadata
        .insert("status_code".to_string(), "200".to_string());
    call.metadata
        .insert("sse_event_count".to_string(), "2".to_string());
    call.metadata
        .insert("call_kind".to_string(), "main".to_string());

    store
        .complete_pending(&GenAISemanticEvent::LLMCall(call))
        .unwrap();

    let conn = store.conn.lock().unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM genai_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 1, "complete must update the idle snapshot row");

    let (status, call_id, trace_id, origin): (String, String, String, String) = conn
        .query_row(
            "SELECT status, call_id, trace_id, pending_origin FROM genai_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(status, "complete");
    assert_eq!(call_id, "real-response-id");
    assert_eq!(trace_id, "real-response-id");
    assert_eq!(origin, "idle_drain");
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_mark_interrupted_stale() {
    let (store, path) = create_populated_store("mark_stale");
    // call-5 is pending at BASE_NS + 4s, well in the past relative to now
    let updated = store.mark_interrupted_stale(1).unwrap();
    assert_eq!(updated, 1);
    let conn = store.conn.lock().unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM genai_events WHERE call_id = 'call-5'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "interrupted");
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_list_pending_for_pid() {
    let (store, path) = create_populated_store("pend_pid");
    let r = store.list_pending_for_pid(200).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, "call-5");
    cleanup_db(&path);
}

#[test]
fn test_list_pending_for_pids() {
    let (store, path) = create_populated_store("pend_pids");
    let r = store.list_pending_for_pids(&[200]).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, "call-5");
    assert!(store.list_pending_for_pids(&[]).unwrap().is_empty());
    cleanup_db(&path);
}

#[test]
fn test_mark_pending_interrupted_for_pid() {
    let (store, path) = create_populated_store("mark_pid");
    let n = store
        .mark_pending_interrupted_for_pid(200, "agent_crash")
        .unwrap();
    assert_eq!(n, 1);
    let conn = store.conn.lock().unwrap();
    let (st, it): (String, String) = conn
        .query_row(
            "SELECT status, interruption_type FROM genai_events WHERE call_id = 'call-5'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(st, "interrupted");
    assert_eq!(it, "agent_crash");
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_crash_sweep_ignores_idle_drain_pending() {
    let path =
        std::env::temp_dir().join(format!("test_genai_idle_sweep_{}.db", std::process::id()));
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();

    for (call_id, origin) in [
        ("dead-drain", PendingOrigin::DeadPidDrain),
        ("idle-drain", PendingOrigin::IdleDrain),
    ] {
        let info = PendingCallInfo {
            call_id: call_id.to_string(),
            trace_id: None,
            conversation_id: Some("c-sweep".to_string()),
            session_id: Some("s-sweep".to_string()),
            start_timestamp_ns: BASE_NS as u64,
            pid: 42,
            process_name: "claude".to_string(),
            agent_name: Some("claude".to_string()),
            http_method: Some("POST".to_string()),
            http_path: Some("/v1/messages".to_string()),
            input_messages: None,
            system_instructions: None,
            user_query: Some("hello".to_string()),
            is_sse: true,
            model: Some("claude-sonnet".to_string()),
            provider: Some("anthropic".to_string()),
            call_kind: "main".to_string(),
            pending_origin: origin,
            pending_match_key: Some(format!("match-{call_id}")),
            process_type: None,
        };
        store.insert_pending(&info).unwrap();
    }

    let listed = store.list_pending_for_pid(42).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "dead-drain");

    let updated = store
        .mark_pending_interrupted_for_pid(42, "agent_crash")
        .unwrap();
    assert_eq!(updated, 1);

    let conn = store.conn.lock().unwrap();
    let idle_status: String = conn
        .query_row(
            "SELECT status FROM genai_events WHERE call_id = 'idle-drain'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(idle_status, "pending");
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_enrich_pending_from_sse() {
    let (store, path) = create_populated_store("enrich_sse");
    let e = SseEnrichment {
        model: Some("gpt-4-turbo".to_string()),
        trace_id: Some("trace-enriched".to_string()),
        provider: Some("openai-e".to_string()),
        output_messages: Some(r#"[{"role":"assistant"}]"#.to_string()),
        sse_event_count: Some(42),
        input_tokens: Some(999),
        output_tokens: Some(888),
    };
    store.enrich_pending_from_sse("call-5", &e).unwrap();
    let conn = store.conn.lock().unwrap();
    let (model, tid, it, ot): (String, String, i64, i64) = conn
        .query_row(
            "SELECT model, trace_id, input_tokens, output_tokens \
             FROM genai_events WHERE call_id = 'call-5'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(model, "gpt-4-turbo");
    assert_eq!(tid, "trace-enriched");
    assert_eq!(it, 999);
    assert_eq!(ot, 888);
    drop(conn);
    cleanup_db(&path);
}

// ─── schema.rs tests ──────────────────────────────────────────────────────────

#[test]
fn test_check_and_prune_if_needed_below_threshold() {
    let (store, path) = create_populated_store("prune_check");
    // Tiny test DB is well below the 200 MB default threshold
    store.check_and_prune_if_needed().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM genai_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 6); // no pruning occurred
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_prune_old_records() {
    let (store, path) = create_populated_store("prune");
    store.prune_old_records().unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM genai_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 5); // 5% of 6 ≈ 1 record deleted
    drop(conn);
    cleanup_db(&path);
}

#[test]
fn test_wal_checkpoint_methods() {
    let (store, path) = create_populated_store("wal_ckpt");
    store.checkpoint().unwrap();
    store.wal_checkpoint().unwrap();
    cleanup_db(&path);
}

// ─── list_sessions / list_traces call_kind filter tests ──────────────────────

fn make_store_with_pending(records: &[(&str, &str, &str, &str, i64)]) -> GenAISqliteStore {
    let path = std::env::temp_dir().join(format!(
        "test_ck_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = GenAISqliteStore::new_with_path(&path).unwrap();
    for (call_id, sid, cid, kind, ts) in records {
        let info = PendingCallInfo {
            call_id: call_id.to_string(),
            trace_id: None,
            conversation_id: Some(cid.to_string()),
            session_id: Some(sid.to_string()),
            start_timestamp_ns: *ts as u64,
            pid: 1,
            process_name: "test".to_string(),
            agent_name: Some("test-agent".to_string()),
            http_method: None,
            http_path: None,
            input_messages: None,
            system_instructions: None,
            user_query: None,
            is_sse: false,
            model: Some("gpt-4".to_string()),
            provider: Some("openai".to_string()),
            call_kind: kind.to_string(),
            pending_origin: PendingOrigin::RequestCapture,
            pending_match_key: None,
            process_type: None,
        };
        store.insert_pending(&info).unwrap();
    }
    store
}

#[test]
fn test_list_sessions_excludes_auxiliary() {
    let store = make_store_with_pending(&[
        ("c1", "sess-a", "conv-1", "main", 1000),
        ("c2", "sess-a", "conv-2", "recap", 2000),
        ("c3", "sess-b", "conv-3", "web_search", 3000),
        ("c4", "sess-b", "conv-4", "main", 4000),
    ]);
    let sessions = store.list_sessions(0, 10000, false).unwrap();
    assert_eq!(sessions.len(), 2);
    for s in &sessions {
        assert!(s.conversation_count >= 1);
    }
    let sessions_all = store.list_sessions(0, 10000, true).unwrap();
    assert_eq!(sessions_all.len(), 2);
    let total_convs: i64 = sessions_all.iter().map(|s| s.conversation_count).sum();
    assert_eq!(total_convs, 4);
}

#[test]
fn test_list_sessions_only_auxiliary_hidden() {
    let store = make_store_with_pending(&[
        ("c1", "sess-recap", "conv-1", "recap", 1000),
        ("c2", "sess-recap", "conv-2", "web_search", 2000),
    ]);
    let sessions = store.list_sessions(0, 10000, false).unwrap();
    assert!(
        sessions.is_empty(),
        "sessions with only auxiliary calls should be hidden"
    );
    let sessions_all = store.list_sessions(0, 10000, true).unwrap();
    assert_eq!(sessions_all.len(), 1);
}

#[test]
fn test_list_sessions_preview_honors_call_kind_filter() {
    let store = make_store_with_pending(&[
        ("c1", "sess-a", "conv-1", "recap", 1000),
        ("c2", "sess-a", "conv-2", "main", 2000),
    ]);
    // Give both calls a user_query: the auxiliary one must not become the
    // preview when auxiliary calls are excluded.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE genai_events SET user_query = 'recap query' WHERE call_id = 'c1'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE genai_events SET user_query = 'main query' WHERE call_id = 'c2'",
            [],
        )
        .unwrap();
    }
    let sessions = store.list_sessions(0, 10000, false).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].first_user_query.as_deref(), Some("main query"));
    assert_eq!(sessions[0].last_user_query.as_deref(), Some("main query"));

    // With auxiliary included, the earlier recap query becomes the first.
    let sessions_all = store.list_sessions(0, 10000, true).unwrap();
    assert_eq!(
        sessions_all[0].first_user_query.as_deref(),
        Some("recap query")
    );
    assert_eq!(
        sessions_all[0].last_user_query.as_deref(),
        Some("main query")
    );
}

#[test]
fn test_list_traces_by_session_excludes_auxiliary() {
    let store = make_store_with_pending(&[
        ("c1", "sess-x", "conv-main", "main", 1000),
        ("c2", "sess-x", "conv-recap", "recap", 2000),
        ("c3", "sess-x", "conv-main", "main", 3000),
    ]);
    let traces = store
        .list_traces_by_session("sess-x", None, None, false)
        .unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].conversation_id, "conv-main");
    assert_eq!(traces[0].call_count, 2);
    let traces_all = store
        .list_traces_by_session("sess-x", None, None, true)
        .unwrap();
    assert_eq!(traces_all.len(), 2);
}

#[test]
fn test_list_traces_call_kind_filter_with_time_range() {
    let store = make_store_with_pending(&[
        ("c1", "sess-t", "conv-a", "main", 1000),
        ("c2", "sess-t", "conv-b", "recap", 2000),
        ("c3", "sess-t", "conv-c", "main", 5000),
    ]);
    let traces = store
        .list_traces_by_session("sess-t", Some(0), Some(3000), false)
        .unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].conversation_id, "conv-a");
    let traces_all = store
        .list_traces_by_session("sess-t", Some(0), Some(3000), true)
        .unwrap();
    assert_eq!(traces_all.len(), 2);
}

// ─── poison-recovery tests ─────────────────────────────────────────────────

/// After intentionally poisoning the conn mutex, methods that use
/// `unwrap_or_else(|e| e.into_inner())` should still operate correctly.
#[test]
fn poison_recovery_conn_still_operational() {
    let (store, path) = create_populated_store("poison_conn");

    // Intentionally poison the conn mutex by panicking while holding the lock
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = store.conn.lock().unwrap();
        panic!("intentional poison");
    }));
    assert!(result.is_err(), "Mutex should be poisoned");

    // Exercise the poison-recovery path: get_events_by_session locks conn
    // and should recover via unwrap_or_else(|e| e.into_inner())
    let events = store.get_events_by_session("sess-1").unwrap();
    assert!(
        !events.is_empty(),
        "Should still read after conn poison recovery"
    );

    // Also exercise a write path (schema.rs: wal_checkpoint via VACUUM)
    store.wal_checkpoint().unwrap();

    cleanup_db(&path);
}

/// After intentionally poisoning the pending and last_flush mutexes,
/// flush() should still operate correctly via poison recovery.
#[test]
fn poison_recovery_flush_still_operational() {
    let (store, path) = create_populated_store("poison_flush");

    // Insert a pending event (normal path) to populate the pending buffer
    let info = PendingCallInfo {
        call_id: "poison-flush-1".to_string(),
        trace_id: None,
        conversation_id: Some("c-pf".to_string()),
        session_id: Some("s-pf".to_string()),
        start_timestamp_ns: BASE_NS as u64,
        pid: 99,
        process_name: "test-proc".to_string(),
        agent_name: Some("test-agent".to_string()),
        http_method: None,
        http_path: None,
        input_messages: None,
        system_instructions: None,
        user_query: Some("hello".to_string()),
        is_sse: false,
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        call_kind: "main".to_string(),
        pending_origin: PendingOrigin::RequestCapture,
        pending_match_key: None,
        process_type: None,
    };
    store.insert_pending(&info).unwrap();

    // Also add to the pending event buffer (for flush)
    {
        let mut pending = store.pending.lock().unwrap();
        use crate::genai::semantic::{GenAISemanticEvent, LLMCall, LLMRequest};
        let call = LLMCall::new(
            "poison-flush-2".to_string(),
            BASE_NS as u64,
            "openai".to_string(),
            "gpt-4".to_string(),
            LLMRequest {
                messages: vec![],
                temperature: None,
                max_tokens: None,
                frequency_penalty: None,
                presence_penalty: None,
                top_p: None,
                top_k: None,
                seed: None,
                stop_sequences: None,
                stream: false,
                tools: None,
                raw_body: None,
            },
            99,
            "test-agent".to_string(),
        );
        pending.push(GenAISemanticEvent::LLMCall(call));
    }

    // Poison both the pending and last_flush mutexes
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g1 = store.pending.lock().unwrap();
        let _g2 = store.last_flush.lock().unwrap();
        panic!("intentional poison");
    }));
    assert!(result.is_err(), "Mutexes should be poisoned");

    // flush() exercises:
    //   - pending.lock().unwrap_or_else(|e| e.into_inner())  (mod.rs)
    //   - last_flush.lock().unwrap_or_else(|e| e.into_inner())  (mod.rs)
    store.flush();

    cleanup_db(&path);
}

#[test]
fn test_token_usage_by_process_type() {
    let path = std::env::temp_dir().join(format!(
        "test_token_by_ptype_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = GenAISqliteStore::new_with_path(&path).unwrap();

    // Insert events with different process_types
    let mut call1 = LLMCall::new(
        "c1".into(),
        1000,
        "openai".into(),
        "gpt-4".into(),
        LLMRequest {
            messages: vec![],
            temperature: None,
            max_tokens: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: None,
        },
        100,
        "agent".into(),
    );
    call1.process_type = Some("agent".into());
    call1.token_usage = Some(crate::genai::semantic::TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    call1.end_timestamp_ns = 2000;
    call1.duration_ns = 1000;
    store
        .store_event(&GenAISemanticEvent::LLMCall(call1))
        .unwrap();

    let mut call2 = LLMCall::new(
        "c2".into(),
        1500,
        "openai".into(),
        "gpt-4".into(),
        LLMRequest {
            messages: vec![],
            temperature: None,
            max_tokens: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: None,
        },
        200,
        "tool".into(),
    );
    call2.process_type = Some("tool".into());
    call2.token_usage = Some(crate::genai::semantic::TokenUsage {
        input_tokens: 30,
        output_tokens: 10,
        total_tokens: 40,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    call2.end_timestamp_ns = 2500;
    call2.duration_ns = 1000;
    store
        .store_event(&GenAISemanticEvent::LLMCall(call2))
        .unwrap();

    // Insert a pending-status event that should be EXCLUDED by the query's
    // `status = 'complete'` filter. Without this negative case, deleting the
    // filter from the SQL would not cause the test to fail.
    let pending = super::pending::PendingCallInfo {
        call_id: "pending-1".into(),
        trace_id: None,
        conversation_id: None,
        session_id: None,
        start_timestamp_ns: 2000,
        pid: 300,
        process_name: "agent".into(),
        agent_name: Some("pending-agent".into()),
        http_method: Some("POST".into()),
        http_path: Some("/v1/chat".into()),
        is_sse: true,
        input_messages: None,
        system_instructions: None,
        user_query: None,
        model: None,
        provider: None,
        call_kind: "main".into(),
        pending_origin: PendingOrigin::RequestCapture,
        pending_match_key: None,
        process_type: Some("agent".into()),
    };
    store.insert_pending(&pending).unwrap();

    let result = store.token_usage_by_process_type(0, 10000).unwrap();
    assert_eq!(
        result.len(),
        2,
        "pending row must be excluded by status='complete' filter"
    );
    // Sorted by total_tokens DESC: agent first
    assert_eq!(result[0].0, "agent");
    assert_eq!(
        result[0].1, 1,
        "call_count for agent must be 1 (pending row excluded)"
    );
    assert_eq!(result[0].4, 150); // total_tokens
    assert_eq!(result[1].0, "tool");
    assert_eq!(result[1].1, 1);
    assert_eq!(result[1].4, 40);

    std::fs::remove_file(&path).ok();
}

// ─── token_usage_by_process_type follow-up tests (PR #661) ────────────────────

/// Build a completed LLMCall with the given process_type and token usage,
/// mirroring the construction pattern of test_token_usage_by_process_type.
fn make_ptype_call(
    call_id: &str,
    start_ns: u64,
    process_type: Option<&str>,
    tokens: (u32, u32, u32),
) -> LLMCall {
    let mut call = LLMCall::new(
        call_id.to_string(),
        start_ns,
        "openai".to_string(),
        "gpt-4".to_string(),
        LLMRequest {
            messages: vec![],
            temperature: None,
            max_tokens: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: None,
        },
        100,
        "agent".to_string(),
    );
    call.process_type = process_type.map(str::to_string);
    call.token_usage = Some(crate::genai::semantic::TokenUsage {
        input_tokens: tokens.0,
        output_tokens: tokens.1,
        total_tokens: tokens.2,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    call.end_timestamp_ns = start_ns + 1000;
    call.duration_ns = 1000;
    call
}

fn temp_db_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// Legacy rows written before the v9 migration have process_type = NULL.
/// The aggregation must fold them into an 'unknown' group via COALESCE
/// instead of dropping them or returning a NULL group key.
#[test]
fn test_token_by_type_null_process_type_coalesce() {
    let path = temp_db_path("test_ptype_coalesce");
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();

    // NULL process_type (legacy row) + one classified 'agent' row
    store
        .store_event(&GenAISemanticEvent::LLMCall(make_ptype_call(
            "c-null",
            1000,
            None,
            (10, 5, 15),
        )))
        .unwrap();
    store
        .store_event(&GenAISemanticEvent::LLMCall(make_ptype_call(
            "c-agent",
            2000,
            Some("agent"),
            (100, 50, 150),
        )))
        .unwrap();

    let result = store.token_usage_by_process_type(0, 10_000).unwrap();
    assert_eq!(
        result.len(),
        2,
        "NULL process_type must form its own 'unknown' group, not vanish"
    );
    let unknown = result
        .iter()
        .find(|r| r.0 == "unknown")
        .expect("NULL process_type rows must be COALESCEd into 'unknown'");
    assert_eq!(unknown.1, 1); // call_count
    assert_eq!(unknown.2, 10); // input_tokens
    assert_eq!(unknown.3, 5); // output_tokens
    assert_eq!(unknown.4, 15); // total_tokens
    let agent = result.iter().find(|r| r.0 == "agent").unwrap();
    assert_eq!(agent.4, 150);

    cleanup_db(&path);
}

/// Rows must be returned sorted by aggregated total_tokens in descending
/// order (ORDER BY ... DESC), regardless of insertion order.
#[test]
fn test_token_by_type_order_desc() {
    let path = temp_db_path("test_ptype_order");
    cleanup_db(&path);
    let store = GenAISqliteStore::new_with_path(&path).unwrap();

    // Insert in ascending token order to rule out incidental ordering
    store
        .store_event(&GenAISemanticEvent::LLMCall(make_ptype_call(
            "c-tool",
            1000,
            Some("tool"),
            (30, 10, 40),
        )))
        .unwrap();
    store
        .store_event(&GenAISemanticEvent::LLMCall(make_ptype_call(
            "c-agent",
            2000,
            Some("agent"),
            (100, 50, 150),
        )))
        .unwrap();
    store
        .store_event(&GenAISemanticEvent::LLMCall(make_ptype_call(
            "c-sub",
            3000,
            Some("sub_agent"),
            (400, 100, 500),
        )))
        .unwrap();

    let result = store.token_usage_by_process_type(0, 10_000).unwrap();
    assert_eq!(result.len(), 3);
    let order: Vec<&str> = result.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        order,
        vec!["sub_agent", "agent", "tool"],
        "rows must be sorted by total_tokens DESC"
    );
    assert_eq!(result[0].4, 500);
    assert_eq!(result[1].4, 150);
    assert_eq!(result[2].4, 40);

    cleanup_db(&path);
}

/// Upgrade path: a v8-era database (all columns up to pending_match_key,
/// no process_type) must be migrated by init_tables (via ensure_col!,
/// schema.rs v9 block) so that:
///   1. the process_type column exists afterwards,
///   2. legacy rows keep process_type = NULL and stay readable,
///   3. the aggregation folds them into the 'unknown' group,
///   4. re-running the migration is idempotent.
#[test]
fn test_schema_v9_migration_from_v8() {
    let path = temp_db_path("test_v8_to_v9");
    cleanup_db(&path);

    // Hand-build a v8-shape database: the base CREATE TABLE from
    // schema.rs init_tables (columns id..created_at, which already contains
    // every v2-v8 column except tool_call_ids) plus the v5 tool_call_ids
    // column added by ensure_col!. Only the v9 process_type column is absent.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE genai_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'complete',
                call_id TEXT,
                trace_id TEXT,
                conversation_id TEXT,
                session_id TEXT,
                instance TEXT,
                start_timestamp_ns INTEGER NOT NULL,
                end_timestamp_ns INTEGER,
                duration_ns INTEGER,
                pid INTEGER,
                process_name TEXT,
                agent_name TEXT,
                operation_name TEXT,
                provider TEXT,
                model TEXT,
                request_model TEXT,
                response_model TEXT,
                temperature REAL,
                max_tokens INTEGER,
                top_p REAL,
                frequency_penalty REAL,
                presence_penalty REAL,
                finish_reasons TEXT,
                server_address TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                total_tokens INTEGER,
                cache_creation_tokens INTEGER,
                cache_read_tokens INTEGER,
                system_instructions TEXT,
                input_messages TEXT,
                output_messages TEXT,
                user_query TEXT,
                http_method TEXT,
                http_path TEXT,
                status_code INTEGER,
                is_sse INTEGER,
                sse_event_count INTEGER,
                interruption_type TEXT,
                call_kind TEXT NOT NULL DEFAULT 'main',
                pending_origin TEXT NOT NULL DEFAULT 'request_capture',
                pending_match_key TEXT,
                tool_call_ids TEXT,
                event_json TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO genai_events (
                event_type, status, call_id, start_timestamp_ns,
                input_tokens, output_tokens, total_tokens, event_json
             ) VALUES ('llm_call', 'complete', 'legacy-1', 1000, 10, 5, 15, '{}')",
            [],
        )
        .unwrap();
    }

    // Real migration entry point: new_with_path -> init_tables -> ensure_col!
    let store = GenAISqliteStore::new_with_path(&path).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('genai_events') \
                 WHERE name = 'process_type'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 1, "v9 migration must add process_type column");
        let pt: Option<String> = conn
            .query_row(
                "SELECT process_type FROM genai_events WHERE call_id = 'legacy-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(pt.is_none(), "legacy row must keep process_type = NULL");
    }

    // Legacy data must flow through the aggregation as 'unknown'
    let result = store.token_usage_by_process_type(0, 10_000).unwrap();
    assert_eq!(result, vec![("unknown".to_string(), 1, 10, 5, 15)]);
    drop(store);

    // Idempotency: opening again re-runs init_tables on a v9 database
    let store2 = GenAISqliteStore::new_with_path(&path)
        .expect("second migration run must not fail (idempotent)");
    {
        let conn = store2.conn.lock().unwrap();
        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('genai_events') \
                 WHERE name = 'process_type'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 1, "re-migration must not duplicate the column");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM genai_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "re-migration must not touch existing rows");
    }

    cleanup_db(&path);
}
