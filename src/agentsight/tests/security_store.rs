use std::fs;

use agentsight::security::{
    ContainmentAction, ContainmentFailureStage, ContainmentLifecycle, SecurityEventFilter,
    SecurityStore, SecurityStoreError,
};
use agentsight::storage::Storage;
use agentsight_enforcement_protocol::{
    Effect, EventIdentity, FileAction, PolicyDecision, PolicyMode, SecurityEvent, SecurityEventKind,
};
use uuid::Uuid;

fn containment_action(lifecycle_state: ContainmentLifecycle) -> ContainmentAction {
    ContainmentAction {
        action_id: Uuid::new_v4(),
        case_id: Uuid::new_v4(),
        binding_id: Uuid::new_v4(),
        agent_id: "hermes-test".into(),
        root_pid: 4242,
        process_start_time: 99,
        source_path: "/home/test/.ssh/id_rsa".into(),
        duration_secs: Some(900),
        expires_at_ns: Some(1_000),
        lifecycle_state,
        blocked_at_ns: None,
        requested_by: "dashboard-token".into(),
        failure_stage: None,
        failure_reason: None,
        attempt_count: 0,
        next_retry_at_ns: None,
        created_at_ns: 100,
        updated_at_ns: 100,
    }
}

fn security_db_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("agentsight-{label}-{}.db", Uuid::new_v4()))
}

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

    assert!(
        store
            .insert_event(&event)
            .expect("first insert should work")
    );
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
        storage
            .security()
            .event(event.event_id)
            .expect("query should work"),
        Some(event)
    );
}

#[test]
fn containment_action_round_trips_and_latest_action_is_found() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    let older = containment_action(ContainmentLifecycle::Expired);
    let mut action = containment_action(ContainmentLifecycle::Pending);
    action.case_id = older.case_id;
    action.created_at_ns = older.created_at_ns + 1;
    action.updated_at_ns = action.created_at_ns;

    store
        .insert_containment_action(&older)
        .expect("older action should insert");
    store
        .insert_containment_action(&action)
        .expect("action should insert");

    assert_eq!(
        store
            .containment_action(action.action_id)
            .expect("action query should work"),
        Some(action.clone())
    );
    assert_eq!(
        store
            .latest_containment_action(action.case_id)
            .expect("latest action query should work"),
        Some(action)
    );
}

#[test]
fn containment_action_updates_all_mutable_state() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    let mut action = containment_action(ContainmentLifecycle::Pending);
    store
        .insert_containment_action(&action)
        .expect("action should insert");

    action.lifecycle_state = ContainmentLifecycle::Expiring;
    action.failure_stage = Some(ContainmentFailureStage::Detach);
    action.failure_reason = Some("enforcer temporarily unavailable".into());
    action.attempt_count = 2;
    action.next_retry_at_ns = Some(750);
    action.updated_at_ns = 500;
    store
        .update_containment_action(&action)
        .expect("action should update");

    assert_eq!(
        store
            .containment_action(action.action_id)
            .expect("action query should work"),
        Some(action)
    );
}

#[test]
fn mark_containment_blocked_preserves_the_first_timestamp() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    let action = containment_action(ContainmentLifecycle::Active);
    store
        .insert_containment_action(&action)
        .expect("action should insert");

    store
        .mark_containment_blocked(action.binding_id, 500)
        .expect("first block should update");
    store
        .mark_containment_blocked(action.binding_id, 800)
        .expect("duplicate block should be idempotent");

    assert_eq!(
        store
            .containment_action(action.action_id)
            .expect("action query should work")
            .expect("action should exist")
            .blocked_at_ns,
        Some(500)
    );
}

#[test]
fn due_containment_actions_include_only_actionable_temporary_rows() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");

    let mut due_active = containment_action(ContainmentLifecycle::Active);
    due_active.expires_at_ns = Some(500);
    let mut future_active = containment_action(ContainmentLifecycle::Active);
    future_active.expires_at_ns = Some(501);
    let mut persistent = containment_action(ContainmentLifecycle::Active);
    persistent.duration_secs = None;
    persistent.expires_at_ns = None;
    let mut pending = containment_action(ContainmentLifecycle::Pending);
    pending.next_retry_at_ns = None;
    let mut due_retry = containment_action(ContainmentLifecycle::Expiring);
    due_retry.next_retry_at_ns = Some(500);
    let mut future_retry = containment_action(ContainmentLifecycle::Expiring);
    future_retry.next_retry_at_ns = Some(501);
    let mut expired = containment_action(ContainmentLifecycle::Expired);
    expired.expires_at_ns = Some(100);
    let mut failed = containment_action(ContainmentLifecycle::Failed);
    failed.next_retry_at_ns = Some(100);

    for action in [
        &due_active,
        &future_active,
        &persistent,
        &pending,
        &due_retry,
        &future_retry,
        &expired,
        &failed,
    ] {
        store
            .insert_containment_action(action)
            .expect("action should insert");
    }

    let due = store
        .due_containment_actions(500, 10)
        .expect("due action query should work");
    let due_ids = due
        .iter()
        .map(|action| action.action_id)
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(due_ids.len(), 3);
    assert!(due_ids.contains(&due_active.action_id));
    assert!(due_ids.contains(&pending.action_id));
    assert!(due_ids.contains(&due_retry.action_id));
}

#[test]
fn containment_queries_reject_unknown_persisted_enums() {
    let path = security_db_path("invalid-containment-enum");
    let action = containment_action(ContainmentLifecycle::Failed);
    {
        let store = SecurityStore::open(&path).expect("fixture store should open");
        store
            .insert_containment_action(&action)
            .expect("action should insert");
    }
    {
        let conn = rusqlite::Connection::open(&path).expect("fixture database should open");
        conn.execute(
            "UPDATE containment_actions SET lifecycle_state = 'unknown' WHERE action_id = ?1",
            [action.action_id.to_string()],
        )
        .expect("fixture row should mutate");
    }

    let store = SecurityStore::open(&path).expect("fixture store should reopen");
    let error = store
        .containment_action(action.action_id)
        .expect_err("unknown lifecycle must fail");

    assert!(matches!(error, SecurityStoreError::InvalidData(_)));
    drop(store);
    {
        let conn = rusqlite::Connection::open(&path).expect("fixture database should open");
        conn.execute(
            "UPDATE containment_actions
             SET lifecycle_state = 'failed', failure_stage = 'unknown'
             WHERE action_id = ?1",
            [action.action_id.to_string()],
        )
        .expect("fixture row should mutate");
    }
    let store = SecurityStore::open(&path).expect("fixture store should reopen");
    let error = store
        .containment_action(action.action_id)
        .expect_err("unknown failure stage must fail");
    assert!(matches!(error, SecurityStoreError::InvalidData(_)));
    drop(store);
    fs::remove_file(path).expect("fixture database should be removed");
}

#[test]
fn containment_writes_reject_unsigned_values_above_sqlite_range() {
    let store = SecurityStore::open_in_memory().expect("fixture store should open");
    let mut action = containment_action(ContainmentLifecycle::Pending);
    action.process_start_time = u64::MAX;

    let error = store
        .insert_containment_action(&action)
        .expect_err("out-of-range value must fail");

    assert!(matches!(error, SecurityStoreError::TimestampOutOfRange(value) if value == u64::MAX));
}
