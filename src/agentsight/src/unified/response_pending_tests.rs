//! Parser-to-storage lifecycle regressions without loading or attaching probes.

use super::*;
use crate::aggregator::{AggregatedResult, ConnectionState};
use crate::config::RuntimeLimits;
use crate::probes::sslsniff::SslEvent;
use crate::storage::sqlite::PendingOrigin;

const PID: u32 = 3_999_989;

fn feed(aggregator: &mut Aggregator, rw: i32, bytes: &[u8]) -> Vec<AggregatedResult> {
    let event = SslEvent {
        source: 0,
        timestamp_ns: 1_000_000_000,
        delta_ns: 0,
        pid: PID,
        tid: PID,
        uid: 0,
        len: bytes.len() as u32,
        rw,
        comm: "fixture".into(),
        buf: bytes.to_vec(),
        is_handshake: false,
        ssl_ptr: 123,
    };
    aggregator.process_result(Parser::new().parse_event(Event::Ssl(event)))
}

fn fixture(prefix: &[u8]) -> Aggregator {
    let mut aggregator = Aggregator::with_limits(
        4,
        &RuntimeLimits {
            connection_idle_timeout_secs: 0,
            ..Default::default()
        },
    );
    let body = r#"{"model":"gpt-4","messages":[{"role":"user","content":"hello"}]}"#;
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: api.openai.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert!(feed(&mut aggregator, 1, request.as_bytes()).is_empty());
    assert!(feed(&mut aggregator, 0, prefix).is_empty());
    aggregator
}

fn prefixes() -> [&'static [u8]; 3] {
    [
        b"HTTP/1.1 200 OK\r\nContent-Typ",
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{",
    ]
}

#[test]
fn response_pending_exit_and_dead_pid_preserve_crash_evidence() {
    for prefix in prefixes() {
        for dead_pid in [false, true] {
            for exit_status in [0, 9] {
                let mut aggregator = fixture(prefix);
                // Periodic eviction must not race either persistence path.
                aggregator.http_mut().evict_idle_and_oversized();
                let drained = if dead_pid {
                    assert!(!crate::utils::procfs::proc_pid(PID).exists());
                    aggregator.drain_dead_pid_connections()
                } else {
                    aggregator.drain_connections_for_pid(PID)
                };
                assert_eq!(drained.len(), 1);
                let (id, state) = &drained[0];
                assert!(matches!(state, ConnectionState::ResponsePending { .. }));
                let pending = GenAIBuilder::new()
                    .build_pending_from_request(
                        state.pending_request().expect("retained request"),
                        id,
                        &ResponseSessionMapper::new(),
                        &HashMap::new(),
                    )
                    .expect("pending LLM call");
                let dir = super::tests::unique_tmp_dir("response-pending");
                let store = GenAISqliteStore::new_with_path(&dir.join("genai.db")).unwrap();
                let interruptions =
                    InterruptionStore::new_with_path(&dir.join("interruptions.db")).unwrap();
                store.insert_pending(&pending).unwrap();
                let rows = store.list_pending_for_pids(&[PID as i32]).unwrap();
                assert_eq!(rows.len(), 1);
                record_agent_crash_interruptions(
                    PID,
                    "fixture",
                    ProcessExitStatus::decode(exit_status),
                    &rows,
                    &interruptions,
                    Some(&store),
                );
                let crashes = interruptions
                    .list(0, i64::MAX, None, Some("agent_crash"), None, None, 100)
                    .unwrap();
                assert_eq!(crashes.len(), usize::from(exit_status == 9));
                assert_eq!(
                    store.list_pending_for_pids(&[PID as i32]).unwrap().len(),
                    usize::from(exit_status == 0)
                );
                std::fs::remove_dir_all(&dir).unwrap();
            }
        }
    }
}

#[test]
fn response_pending_idle_snapshot_persists_once_and_can_resume() {
    let body = r#"{"id":"fixture","model":"gpt-4","choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    for split in [26, response.len() - 1] {
        let mut aggregator = fixture(&response.as_bytes()[..split]);
        aggregator.http_mut().evict_idle_and_oversized();
        let snapshots = aggregator.snapshot_idle_connections();
        assert_eq!(snapshots.len(), 1);
        assert!(aggregator.snapshot_idle_connections().is_empty());
        let (id, state) = &snapshots[0];
        let mut pending = GenAIBuilder::new()
            .build_pending_from_request(
                state.pending_request().expect("retained request"),
                id,
                &ResponseSessionMapper::new(),
                &HashMap::new(),
            )
            .unwrap();
        pending.pending_origin = PendingOrigin::IdleDrain;
        let dir = super::tests::unique_tmp_dir("response-pending");
        let store = GenAISqliteStore::new_with_path(&dir.join("genai.db")).unwrap();
        store.insert_pending(&pending).unwrap();
        // Crash-candidate queries intentionally exclude idle snapshots; inspect
        // the stored row to verify both persistence and in-place reconciliation.
        let db = rusqlite::Connection::open(dir.join("genai.db")).unwrap();
        let row_state = || {
            db.query_row(
                "SELECT COUNT(*), MIN(status), MIN(pending_origin) FROM genai_events",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap()
        };
        assert_eq!(row_state(), (1, "pending".into(), "idle_drain".into()));
        aggregator.http_mut().evict_idle_and_oversized();
        let completed = feed(&mut aggregator, 0, &response.as_bytes()[split..]);
        assert_eq!(completed.len(), 1);
        let analyzed = Analyzer::new().analyze_aggregated(&completed[0]);
        let (output, _) = GenAIBuilder::new().build_with_pending(
            &analyzed,
            &ResponseSessionMapper::new(),
            &HashMap::new(),
        );
        assert!(!output.events.is_empty());
        for event in &output.events {
            store.complete_pending(event).unwrap();
        }
        assert_eq!(row_state(), (1, "complete".into(), "idle_drain".into()));
        assert!(!aggregator.has_pending());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
