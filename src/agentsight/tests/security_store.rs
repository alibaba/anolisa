use agentsight::security::{SecurityEventFilter, SecurityStore, SecurityStoreError};
use agentsight::storage::Storage;
use agentsight_enforcement_protocol::{
    Effect, EventIdentity, FileAction, PolicyDecision, PolicyMode, SecurityEvent,
    SecurityEventKind,
};
use uuid::Uuid;

fn fixture_file_action(path: &str, occurred_at_ns: u64) -> SecurityEvent {
    SecurityEvent {
        event_id: Uuid::new_v4(),
        occurred_at_ns,
        observed_at_ns: occurred_at_ns.saturating_add(1),
        identity: EventIdentity {
            binding_id: Uuid::new_v4(),
            agent_id: "hermes-test".into(),
            agent_name: Some("Hermes".into()),
            session_id: Some("session-1".into()),
            conversation_id: None,
            tool_call_id: Some("tool-call-1".into()),
            pid: 4242,
            process_start_time: 99,
            ppid: Some(42),
            cgroup_id: None,
            protocol_version: agentsight_enforcement_protocol::PROTOCOL_VERSION,
            enforcer_version: "test".into(),
            actplane_revision: "test".into(),
        },
        kind: SecurityEventKind::FileAction(FileAction {
            policy_id: "credential-exfiltration".into(),
            policy_revision: 3,
            operation: "read".into(),
            path: path.into(),
            resource_class: "credential".into(),
            succeeded: true,
            errno: None,
            rule_id: Some("credential-source".into()),
        }),
    }
}

fn fixture_policy_decision(occurred_at_ns: u64, blocked: bool) -> SecurityEvent {
    let mut event = fixture_file_action("~/.ssh/id_rsa", occurred_at_ns);
    event.kind = SecurityEventKind::PolicyDecision(PolicyDecision {
        policy_id: "credential-exfiltration".into(),
        policy_revision: 3,
        source_event_id: Uuid::new_v4(),
        sink_event_id: Uuid::new_v4(),
        mode: PolicyMode::Enforce,
        requested_effect: Effect::Block,
        blocked,
        killed: false,
        errno: blocked.then_some(libc::EPERM),
        risk_score: 85,
        reason: "credential taint reached a public endpoint".into(),
    });
    event
}

#[test]
fn duplicate_event_is_idempotent_and_secret_content_is_absent() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    let event = fixture_file_action("~/.ssh/id_rsa", 100);

    assert!(store.insert_event(&event).expect("first insert should work"));
    assert!(!store.insert_event(&event).expect("duplicate should work"));

    let stored = store
        .event(event.event_id)
        .expect("query should work")
        .expect("event should exist");
    let json = serde_json::to_string(&stored).expect("fixture should serialize");
    assert!(!json.contains("PRIVATE KEY"));
    assert!(json.contains("~/.ssh/id_rsa"));
}

#[test]
fn list_events_clamps_limit_and_orders_newest_first() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    for occurred_at_ns in [100, 300, 200] {
        store
            .insert_event(&fixture_file_action("~/.ssh/id_rsa", occurred_at_ns))
            .expect("fixture event should insert");
    }

    let page = store
        .list_events(&SecurityEventFilter {
            limit: 5_000,
            ..SecurityEventFilter::default()
        })
        .expect("query should work");

    assert_eq!(page.limit, 1_000);
    assert!(
        page.items
            .windows(2)
            .all(|pair| pair[0].occurred_at_ns >= pair[1].occurred_at_ns)
    );
}

#[test]
fn event_filters_use_exact_bound_values() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    let expected = fixture_file_action("~/.ssh/id_rsa", 200);
    let binding_id = expected.identity.binding_id;
    store
        .insert_event(&fixture_file_action("~/.ssh/id_ed25519", 100))
        .expect("fixture event should insert");
    store
        .insert_event(&expected)
        .expect("fixture event should insert");

    let page = store
        .list_events(&SecurityEventFilter {
            start_ns: Some(150),
            end_ns: Some(250),
            event_type: Some("file_action".into()),
            policy_id: Some("credential-exfiltration".into()),
            agent_id: Some("hermes-test".into()),
            session_id: Some("session-1".into()),
            binding_id: Some(binding_id),
            offset: -50,
            ..SecurityEventFilter::default()
        })
        .expect("filtered query should work");

    assert_eq!(page.items, vec![expected]);
    assert_eq!(page.offset, 0);
}

#[test]
fn count_by_rejects_unknown_columns() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");

    let error = store
        .count_by("event_json; DROP TABLE security_events")
        .expect_err("unknown grouping must fail");

    assert!(matches!(error, SecurityStoreError::InvalidFilter(_)));
}

#[test]
fn summary_and_grouping_use_normalized_event_metadata() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    store
        .insert_event(&fixture_file_action("~/.ssh/id_rsa", 100))
        .expect("file event should insert");
    store
        .insert_event(&fixture_policy_decision(200, true))
        .expect("decision event should insert");

    let counts = store.count_by("event_type").expect("grouping should work");
    assert!(
        counts
            .iter()
            .any(|item| item.key == "file_action" && item.count == 1)
    );
    assert!(
        counts
            .iter()
            .any(|item| item.key == "policy_decision" && item.count == 1)
    );

    let summary = store.summary().expect("summary should work");
    assert_eq!(summary.total_events, 2);
    assert_eq!(summary.blocked_events, 1);
    assert_eq!(summary.evidence_loss_events, 0);
}

#[test]
fn unified_storage_exposes_the_security_store() {
    let storage = Storage::noop();
    let event = fixture_file_action("~/.ssh/id_rsa", 100);

    assert!(
        storage
            .security()
            .insert_event(&event)
            .expect("event should insert through unified storage")
    );
    assert_eq!(
        storage.security().event(event.event_id).expect("query should work"),
        Some(event)
    );
}
