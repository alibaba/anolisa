use std::path::Path;

use cosh_gateway_contracts::common::{
    BoundedOpaque, ContractHeader, ContractSchema, Correlation, RuntimeSelector, TargetRef,
};
use cosh_gateway_contracts::ids::{InstallationId, RunId};
use cosh_gateway_contracts::task::TaskEvent;

use super::*;

fn envelope(
    task_id: &TaskId,
    actor_id: &ActorId,
    revision: u64,
    event: TaskEvent,
) -> TaskEventEnvelope {
    let mut correlation = Correlation::new(InstallationId::new());
    correlation.actor_id = Some(actor_id.clone());
    correlation.task_id = Some(task_id.clone());
    TaskEventEnvelope {
        header: ContractHeader::new(
            ContractSchema::TaskEvent,
            MessageId::new(),
            revision,
            correlation,
        ),
        task_id: task_id.clone(),
        revision,
        event,
    }
}

fn submitted(task_id: &TaskId, actor_id: &ActorId) -> TaskEventEnvelope {
    envelope(
        task_id,
        actor_id,
        1,
        TaskEvent::TaskSubmitted {
            intent_digest: Digest::parse("a".repeat(64)).unwrap(),
            target: TargetRef {
                kind: BoundedName::new("local").unwrap(),
                authority: BoundedName::new("test").unwrap(),
                identifier: BoundedOpaque::new("target").unwrap(),
            },
        },
    )
}

fn task_commit(
    task_id: &TaskId,
    actor_id: &ActorId,
    key: &str,
    digest: char,
    events: Vec<TaskEventEnvelope>,
    outbox: Vec<OutboxIntent>,
) -> TaskCommit {
    let _ = task_id;
    TaskCommit {
        actor_id: actor_id.clone(),
        idempotency_key: IdempotencyKey::new(key).unwrap(),
        command_digest: Digest::parse(digest.to_string().repeat(64)).unwrap(),
        expected_revision: Some(events.first().map_or(0, |event| event.revision - 1)),
        events,
        outbox,
        committed_at_ms: 100,
    }
}

fn outbox(event: &TaskEventEnvelope, delivery_id: DeliveryId) -> OutboxIntent {
    OutboxIntent {
        delivery_id,
        event_id: event.header.message_id.clone(),
        delivery_kind: BoundedName::new("task_event").unwrap(),
        payload: serde_json::json!({"event_id": event.header.message_id}),
        next_attempt_at_ms: 100,
    }
}

fn table_count(store: &SqliteTaskStore, table: &str) -> i64 {
    let query = format!("SELECT COUNT(*) FROM {table}");
    store
        .connection()
        .query_row(&query, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn commits_projection_event_receipt_and_outbox_atomically() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let task_id = TaskId::new();
    let actor_id = ActorId::new();
    let event = submitted(&task_id, &actor_id);
    let delivery_id = DeliveryId::new();
    let commit = task_commit(
        &task_id,
        &actor_id,
        "create",
        'a',
        vec![event.clone()],
        vec![outbox(&event, delivery_id.clone())],
    );

    let outcome = store.commit_task(&commit).unwrap();
    let CommitOutcome::Applied(receipt) = outcome else {
        panic!("first commit must be applied")
    };
    assert_eq!(receipt.revision, 1);
    assert_eq!(receipt.delivery_ids, [delivery_id]);
    assert_eq!(store.load_task(&task_id).unwrap().revision(), 1);
    assert_eq!(table_count(&store, "task_events"), 1);
    assert_eq!(table_count(&store, "command_receipts"), 1);
    assert_eq!(table_count(&store, "outbox"), 1);
}

#[test]
fn idempotency_replays_same_digest_and_rejects_conflict() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let task_id = TaskId::new();
    let actor_id = ActorId::new();
    let event = submitted(&task_id, &actor_id);
    let mut commit = task_commit(
        &task_id,
        &actor_id,
        "same-key",
        'b',
        vec![event],
        Vec::new(),
    );

    let applied = store.commit_task(&commit).unwrap();
    assert!(matches!(applied, CommitOutcome::Applied(_)));
    commit.expected_revision = Some(99);
    let replayed = store.commit_task(&commit).unwrap();
    assert!(matches!(replayed, CommitOutcome::Replayed(_)));
    commit.command_digest = Digest::parse("c".repeat(64)).unwrap();
    assert!(matches!(
        store.commit_task(&commit),
        Err(StoreError::IdempotencyConflict)
    ));
    assert_eq!(table_count(&store, "task_events"), 1);
}

#[test]
fn revision_conflict_has_no_partial_rows() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let task_id = TaskId::new();
    let actor_id = ActorId::new();
    let event = submitted(&task_id, &actor_id);
    let mut commit = task_commit(
        &task_id,
        &actor_id,
        "conflict",
        'd',
        vec![event],
        Vec::new(),
    );
    commit.expected_revision = Some(1);

    assert!(matches!(
        store.commit_task(&commit),
        Err(StoreError::RevisionConflict {
            expected: 1,
            actual: 0
        })
    ));
    assert_eq!(table_count(&store, "tasks"), 0);
    assert_eq!(table_count(&store, "task_events"), 0);
    assert_eq!(table_count(&store, "command_receipts"), 0);
}

#[test]
fn actor_substitution_cannot_append_or_create_partial_rows() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let task_id = TaskId::new();
    let owner = ActorId::new();
    let attacker = ActorId::new();
    let event = submitted(&task_id, &owner);
    let substituted_create = task_commit(
        &task_id,
        &attacker,
        "substitute-create",
        '3',
        vec![event],
        Vec::new(),
    );
    assert!(matches!(
        store.commit_task(&substituted_create),
        Err(StoreError::InvalidCommit { .. })
    ));
    assert_eq!(table_count(&store, "tasks"), 0);

    let create_event = submitted(&task_id, &owner);
    store
        .commit_task(&task_commit(
            &task_id,
            &owner,
            "owner-create",
            '4',
            vec![create_event],
            Vec::new(),
        ))
        .unwrap();
    let queued = envelope(
        &task_id,
        &attacker,
        2,
        TaskEvent::TaskQueued {
            run_id: RunId::new(),
            runtime: RuntimeSelector {
                runtime: BoundedName::new("core").unwrap(),
                profile: None,
            },
        },
    );
    assert!(matches!(
        store.commit_task(&task_commit(
            &task_id,
            &attacker,
            "substitute-append",
            '5',
            vec![queued],
            Vec::new(),
        )),
        Err(StoreError::InvalidCommit { .. })
    ));
    assert_eq!(store.load_task(&task_id).unwrap().revision(), 1);
    assert_eq!(table_count(&store, "task_events"), 1);
    assert_eq!(table_count(&store, "command_receipts"), 1);
}

#[test]
fn failed_outbox_insert_rolls_back_task_append() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let task_id = TaskId::new();
    let actor_id = ActorId::new();
    let initial = submitted(&task_id, &actor_id);
    let duplicate_delivery = DeliveryId::new();
    let create = task_commit(
        &task_id,
        &actor_id,
        "create",
        'e',
        vec![initial.clone()],
        vec![outbox(&initial, duplicate_delivery.clone())],
    );
    store.commit_task(&create).unwrap();

    let queued = envelope(
        &task_id,
        &actor_id,
        2,
        TaskEvent::TaskQueued {
            run_id: RunId::new(),
            runtime: RuntimeSelector {
                runtime: BoundedName::new("core").unwrap(),
                profile: None,
            },
        },
    );
    let append = task_commit(
        &task_id,
        &actor_id,
        "queue",
        'f',
        vec![queued.clone()],
        vec![outbox(&queued, duplicate_delivery)],
    );
    assert!(matches!(
        store.commit_task(&append),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(store.load_task(&task_id).unwrap().revision(), 1);
    assert_eq!(table_count(&store, "task_events"), 1);
    assert_eq!(table_count(&store, "command_receipts"), 1);
    assert_eq!(table_count(&store, "outbox"), 1);
}

#[test]
fn recovers_projection_after_durable_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gateway/state.db");
    let task_id = TaskId::new();
    let actor_id = ActorId::new();
    {
        let mut store = SqliteTaskStore::open(&path).unwrap();
        let event = submitted(&task_id, &actor_id);
        let event_id = event.header.message_id.clone();
        store
            .commit_task(&task_commit(
                &task_id,
                &actor_id,
                "recover",
                '1',
                vec![event],
                Vec::new(),
            ))
            .unwrap();
        let mut queued = envelope(
            &task_id,
            &actor_id,
            2,
            TaskEvent::TaskQueued {
                run_id: RunId::new(),
                runtime: RuntimeSelector {
                    runtime: BoundedName::new("core").unwrap(),
                    profile: None,
                },
            },
        );
        queued.header.correlation.causation_message_id = Some(event_id.clone());
        store
            .commit_task(&task_commit(
                &task_id,
                &actor_id,
                "queue-after-recover",
                '2',
                vec![queued],
                Vec::new(),
            ))
            .unwrap();
        let causation: Option<String> = store
            .connection()
            .query_row(
                "SELECT causation_id FROM task_events
                 WHERE task_id = ?1 AND revision = 2",
                params![task_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(causation.as_deref(), Some(event_id.as_str()));
    }

    let store = SqliteTaskStore::open(Path::new(&path)).unwrap();
    let recovered = store.recover_task(&task_id).unwrap();
    assert_eq!(recovered.task_id(), &task_id);
    assert_eq!(recovered.revision(), 2);
    assert_eq!(recovered.state(), TaskState::Queued);
}

#[test]
fn normal_load_and_commit_reject_divergent_snapshot() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let task_id = TaskId::new();
    let actor_id = ActorId::new();
    let create = task_commit(
        &task_id,
        &actor_id,
        "verified-create",
        '6',
        vec![submitted(&task_id, &actor_id)],
        Vec::new(),
    );
    store.commit_task(&create).unwrap();

    let snapshot_json: String = store
        .connection()
        .query_row(
            "SELECT snapshot_json FROM tasks WHERE task_id = ?1",
            params![task_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let mut snapshot: serde_json::Value = serde_json::from_str(&snapshot_json).unwrap();
    snapshot["state"] = serde_json::Value::String("queued".to_string());
    store
        .connection()
        .execute(
            "UPDATE tasks SET snapshot_json = ?2 WHERE task_id = ?1",
            params![task_id.as_str(), serde_json::to_string(&snapshot).unwrap()],
        )
        .unwrap();

    assert!(matches!(
        store.load_task(&task_id),
        Err(StoreError::Corrupt { .. })
    ));
    assert!(matches!(
        store.commit_task(&create),
        Err(StoreError::Corrupt { .. })
    ));

    let queued = envelope(
        &task_id,
        &actor_id,
        2,
        TaskEvent::TaskQueued {
            run_id: RunId::new(),
            runtime: RuntimeSelector {
                runtime: BoundedName::new("core").unwrap(),
                profile: None,
            },
        },
    );
    assert!(matches!(
        store.commit_task(&task_commit(
            &task_id,
            &actor_id,
            "verified-append",
            '7',
            vec![queued],
            Vec::new(),
        )),
        Err(StoreError::Corrupt { .. })
    ));
    assert_eq!(table_count(&store, "task_events"), 1);
    assert_eq!(table_count(&store, "command_receipts"), 1);
}
