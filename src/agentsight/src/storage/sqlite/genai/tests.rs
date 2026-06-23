use super::*;
use crate::genai::semantic::{GenAISemanticEvent, LLMCall, LLMRequest};

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
    let r = store.list_sessions(BASE_NS, BASE_NS + 6 * STEP_NS).unwrap();
    assert_eq!(r.len(), 2);
    // sess-1 last_seen=base+5s > sess-2 base+4s
    assert_eq!(r[0].session_id, "sess-1");
    assert_eq!(r[0].conversation_count, 2);
    assert_eq!(r[0].total_input_tokens, 500);
    assert_eq!(r[1].session_id, "sess-2");
    assert_eq!(r[1].total_input_tokens, 550);
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
    let r = store.list_traces_by_session("sess-1", None, None).unwrap();
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
        .list_traces_by_session("sess-1", Some(BASE_NS), Some(BASE_NS + STEP_NS))
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
