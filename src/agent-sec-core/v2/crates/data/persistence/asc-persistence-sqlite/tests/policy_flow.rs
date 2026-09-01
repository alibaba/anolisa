use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use asc_foundation_types::{ResourceId, Revision};
use asc_pap::{PapError, PapRepository, PapService, ScopeSelector};
use asc_persistence_sqlite::SqlitePolicyStore;
use asc_policy_engine::PolicyTemplate;
use asc_policy_runtime::testing::FakePolicyAdapter;
use asc_policy_runtime::{
    AdapterAccepted, AdapterCommand, AdapterDispatchError, BindingDesiredState, OperationState,
    PolicyAdapter, PolicyRuntime, PreparedBinding, ReconcileOperation, RuntimeError,
    RuntimeRepository, UnavailablePolicyAdapter,
};

#[derive(Default)]
struct RetryableAdapter {
    attempts: AtomicUsize,
}

impl PolicyAdapter for RetryableAdapter {
    fn submit(&self, _command: &AdapterCommand) -> Result<AdapterAccepted, AdapterDispatchError> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Err(AdapterDispatchError::Retryable)
    }
}

fn id(value: &str) -> ResourceId {
    ResourceId::new(value).unwrap()
}

fn revision(value: u64) -> Revision {
    Revision::new(value).unwrap()
}

fn selector() -> ScopeSelector {
    ScopeSelector::Pid { pid: 4242 }
}

fn prepare(store: &Arc<SqlitePolicyStore>) -> (ResourceId, ResourceId) {
    let pap = PapService::new(Arc::clone(store));
    let policy = pap
        .put_policy(
            None,
            "seed-policy",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/secrets/**".to_owned()],
            },
        )
        .unwrap();
    let scope = pap.put_scope(None, &selector()).unwrap();
    (policy.policy_id, scope.scope_id)
}

#[test]
fn immutable_authoring_and_fake_adapter_dispatch_are_durable() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let (policy_id, scope_id) = prepare(&store);
    let adapter = Arc::new(FakePolicyAdapter::default());
    let runtime = PolicyRuntime::new(Arc::clone(&store), Arc::clone(&store), Arc::clone(&adapter));

    let accepted = runtime
        .put_binding(None, &policy_id, revision(1), &scope_id, revision(1))
        .unwrap();
    assert_eq!(accepted.state, OperationState::Queued);
    assert_eq!(
        runtime.dispatch_once().unwrap().unwrap().state,
        OperationState::Dispatched
    );

    let commands = adapter.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].operation_id, accepted.operation_id);
    assert_eq!(commands[0].binding.binding_id, accepted.binding_id);
    assert_eq!(commands[0].binding.policy.policy_id, policy_id);
    assert_eq!(commands[0].binding.scope.scope_id, scope_id);
}

#[test]
fn unavailable_adapter_blocks_without_claiming_enforcement() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let (policy_id, scope_id) = prepare(&store);
    let runtime = PolicyRuntime::new(
        Arc::clone(&store),
        Arc::clone(&store),
        Arc::new(UnavailablePolicyAdapter),
    );
    runtime
        .put_binding(None, &policy_id, revision(1), &scope_id, revision(1))
        .unwrap();
    let blocked = runtime.dispatch_once().unwrap().unwrap();
    assert_eq!(blocked.state, OperationState::Blocked);
    assert_eq!(blocked.error_code.as_deref(), Some("adapter_unavailable"));
    assert!(runtime.dispatch_once().unwrap().is_none());
}

#[test]
fn retryable_dispatch_waits_without_busy_looping() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let (policy_id, scope_id) = prepare(&store);
    let adapter = Arc::new(RetryableAdapter::default());
    let runtime = PolicyRuntime::new(Arc::clone(&store), Arc::clone(&store), Arc::clone(&adapter));
    runtime
        .put_binding(None, &policy_id, revision(1), &scope_id, revision(1))
        .unwrap();

    let waiting = runtime.dispatch_once().unwrap().unwrap();
    assert_eq!(waiting.state, OperationState::RetryWait);
    assert_eq!(waiting.error_code.as_deref(), Some("adapter_retryable"));
    assert!(runtime.dispatch_once().unwrap().is_none());
    assert_eq!(adapter.attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn policy_put_converges_complete_state_and_allocates_revisions() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let pap = PapService::new(Arc::clone(&store));
    let original_template = PolicyTemplate::HighSensitivityReadDeny {
        files: vec!["/secrets/**".to_owned()],
    };
    let first = pap
        .put_policy(None, "protect-secrets", &original_template)
        .unwrap();
    uuid::Uuid::parse_str(first.policy_id.as_str()).unwrap();
    assert_eq!(first.revision, revision(1));

    let repeated = pap
        .put_policy(
            Some(&first.policy_id),
            "protect-secrets",
            &original_template,
        )
        .unwrap();
    assert_eq!(repeated, first);
    assert_eq!(
        pap.repository()
            .list_policies(100, 0, usize::MAX)
            .unwrap()
            .total,
        1
    );
    assert!(matches!(
        pap.repository().list_policies(100, 0, 1),
        Err(PapError::ResponseTooLarge)
    ));

    let second_create = pap
        .put_policy(None, "protect-secrets", &original_template)
        .unwrap();
    assert_ne!(second_create.policy_id, first.policy_id);
    assert_eq!(second_create.revision, revision(1));

    let renamed = pap
        .put_policy(
            Some(&first.policy_id),
            "protect-production-secrets",
            &original_template,
        )
        .unwrap();
    assert_eq!(renamed.revision, revision(2));
    assert_eq!(renamed.policy_name, "protect-production-secrets");

    let changed = pap
        .put_policy(
            Some(&first.policy_id),
            "prevent-destructive-changes",
            &PolicyTemplate::PreventFileDeletion {
                files: vec!["/different/**".to_owned()],
            },
        )
        .unwrap();
    assert_eq!(changed.revision, revision(3));
    assert_eq!(changed.canonical_policy.revision.get(), 3);
    assert_eq!(
        pap.repository()
            .get_policy(&first.policy_id, revision(1))
            .unwrap(),
        first
    );

    let same_name_different_id = pap
        .put_policy(
            None,
            "prevent-destructive-changes",
            &PolicyTemplate::PreventFileDeletion {
                files: vec!["/other/**".to_owned()],
            },
        )
        .unwrap();
    assert_ne!(same_name_different_id.policy_id, first.policy_id);
    assert_eq!(same_name_different_id.revision, revision(1));
    assert_eq!(
        pap.repository()
            .list_policies(100, 0, usize::MAX)
            .unwrap()
            .total,
        5
    );
}

#[test]
fn policy_put_rejects_invalid_human_readable_names() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let pap = PapService::new(store);
    let template = PolicyTemplate::HighSensitivityReadDeny {
        files: vec!["/secrets/**".to_owned()],
    };

    for invalid in ["", "   ", "line\nbreak"] {
        assert!(matches!(
            pap.put_policy(None, invalid, &template),
            Err(PapError::InvalidPolicyName(_))
        ));
    }
    assert!(matches!(
        pap.put_policy(None, &"x".repeat(257), &template),
        Err(PapError::InvalidPolicyName(_))
    ));
    assert!(matches!(
        pap.put_policy(Some(&id("not-a-uuid")), "valid-name", &template),
        Err(PapError::InvalidIdentifier(_))
    ));
}

#[test]
fn policy_put_with_an_unknown_id_is_not_a_create() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let pap = PapService::new(store);
    let unknown = id("6efed5ea-47c9-4b14-8e86-888f2ad88fc7");
    assert!(matches!(
        pap.put_policy(
            Some(&unknown),
            "unknown-policy",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/secrets/**".to_owned()],
            },
        ),
        Err(PapError::NotFound)
    ));
}

#[test]
fn scope_put_uses_daemon_owned_identity_and_never_reuses_deleted_revisions() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let pap = PapService::new(Arc::clone(&store));
    let first = pap
        .put_scope(None, &ScopeSelector::Pid { pid: 4242 })
        .unwrap();
    uuid::Uuid::parse_str(first.scope_id.as_str()).unwrap();
    assert_eq!(first.revision, revision(1));

    let repeated = pap
        .put_scope(Some(&first.scope_id), &ScopeSelector::Pid { pid: 4242 })
        .unwrap();
    assert_eq!(repeated, first);
    assert_eq!(store.list_scopes(100, 0, usize::MAX).unwrap().total, 1);

    let another = pap
        .put_scope(None, &ScopeSelector::Pid { pid: 4242 })
        .unwrap();
    assert_ne!(another.scope_id, first.scope_id);

    let second = pap
        .put_scope(
            Some(&first.scope_id),
            &ScopeSelector::CgroupId { cgroup_id: 99 },
        )
        .unwrap();
    assert_eq!(second.revision, revision(2));
    store
        .delete_scope_revision(&first.scope_id, revision(2))
        .unwrap();

    let third = pap
        .put_scope(
            Some(&first.scope_id),
            &ScopeSelector::CgroupId { cgroup_id: 100 },
        )
        .unwrap();
    assert_eq!(third.revision, revision(3));
    let state = store
        .get_scope_revision_state(&first.scope_id)
        .unwrap()
        .unwrap();
    assert_eq!(state.last_allocated_revision, revision(3));

    let unknown = id("6efed5ea-47c9-4b14-8e86-888f2ad88fc7");
    assert!(matches!(
        pap.put_scope(Some(&unknown), &ScopeSelector::Pid { pid: 7 }),
        Err(PapError::NotFound)
    ));
}

#[test]
fn concurrent_policy_puts_are_serialized_into_internal_revisions() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let pap = PapService::new(Arc::clone(&store));
    let created = pap
        .put_policy(
            None,
            "initial-state",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/initial/**".to_owned()],
            },
        )
        .unwrap();
    let policy_id = created.policy_id;
    let barrier = Arc::new(Barrier::new(3));

    let first_pap = PapService::new(Arc::clone(&store));
    let first_barrier = Arc::clone(&barrier);
    let first_policy_id = policy_id.clone();
    let first = thread::spawn(move || {
        first_barrier.wait();
        first_pap
            .put_policy(
                Some(&first_policy_id),
                "first-desired-state",
                &PolicyTemplate::HighSensitivityReadDeny {
                    files: vec!["/first/**".to_owned()],
                },
            )
            .unwrap()
    });

    let second_pap = PapService::new(Arc::clone(&store));
    let second_barrier = Arc::clone(&barrier);
    let second_policy_id = policy_id.clone();
    let second = thread::spawn(move || {
        second_barrier.wait();
        second_pap
            .put_policy(
                Some(&second_policy_id),
                "second-desired-state",
                &PolicyTemplate::PreventFileDeletion {
                    files: vec!["/second/**".to_owned()],
                },
            )
            .unwrap()
    });

    barrier.wait();
    let mut revisions = [
        first.join().unwrap().revision,
        second.join().unwrap().revision,
    ];
    revisions.sort();
    assert_eq!(revisions, [revision(2), revision(3)]);
    assert_eq!(store.list_policies(100, 0, usize::MAX).unwrap().total, 3);
    assert_eq!(
        store
            .get_policy_revision_state(&policy_id)
            .unwrap()
            .unwrap()
            .latest
            .unwrap()
            .revision,
        revision(3)
    );
}

#[test]
fn source_revisions_can_be_deleted_while_a_binding_snapshot_remains_runnable() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let (policy_id, scope_id) = prepare(&store);
    let pap = PapService::new(Arc::clone(&store));
    let second = pap
        .put_policy(
            Some(&policy_id),
            "second-revision",
            &PolicyTemplate::PreventFileDeletion {
                files: vec!["/important/**".to_owned()],
            },
        )
        .unwrap();
    assert_eq!(second.revision, revision(2));
    let runtime = PolicyRuntime::new(
        Arc::clone(&store),
        Arc::clone(&store),
        Arc::new(UnavailablePolicyAdapter),
    );
    let accepted = runtime
        .put_binding(None, &policy_id, revision(1), &scope_id, revision(1))
        .unwrap();

    let deleted = store
        .delete_policy_revision(&policy_id, revision(2))
        .unwrap();
    assert_eq!(deleted, second);
    assert!(matches!(
        store.get_policy(&policy_id, revision(2)),
        Err(PapError::NotFound)
    ));
    assert!(matches!(
        runtime.put_binding(None, &policy_id, revision(2), &scope_id, revision(1),),
        Err(RuntimeError::Pap(PapError::NotFound))
    ));
    store
        .delete_policy_revision(&policy_id, revision(1))
        .unwrap();
    store.delete_scope_revision(&scope_id, revision(1)).unwrap();
    assert!(matches!(
        store.get_policy(&policy_id, revision(1)),
        Err(PapError::NotFound)
    ));
    assert!(matches!(
        store.get_scope(&scope_id, revision(1)),
        Err(PapError::NotFound)
    ));

    let binding = store.get_binding(&accepted.binding_id).unwrap();
    assert_eq!(binding.policy.policy_id, policy_id);
    assert_eq!(binding.scope.scope_id, scope_id);
    assert_eq!(
        runtime.dispatch_once().unwrap().unwrap().state,
        OperationState::Blocked
    );
    assert!(matches!(
        runtime.put_binding(
            None,
            &binding.policy.policy_id,
            revision(1),
            &binding.scope.scope_id,
            revision(1),
        ),
        Err(RuntimeError::Pap(PapError::NotFound))
    ));
}

#[test]
fn resolved_binding_snapshot_can_commit_after_source_revision_deletion() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let (policy_id, scope_id) = prepare(&store);
    let policy = store.get_policy(&policy_id, revision(1)).unwrap();
    let scope = store.get_scope(&scope_id, revision(1)).unwrap();
    let binding = PreparedBinding {
        binding_id: id("binding-from-stale-snapshot"),
        binding_revision: revision(1),
        policy,
        scope,
        desired_state: BindingDesiredState::Ready,
    };
    let operation = ReconcileOperation {
        operation_id: id("operation-from-stale-snapshot"),
        binding_id: binding.binding_id.clone(),
        binding_revision: binding.binding_revision,
        request_digest: "sha256:stale-snapshot".to_owned(),
        state: OperationState::Queued,
        stage: "dispatch_adapter".to_owned(),
        error_code: None,
    };

    store
        .delete_policy_revision(&policy_id, revision(1))
        .unwrap();
    store.delete_scope_revision(&scope_id, revision(1)).unwrap();
    assert_eq!(
        store.accept_binding(&binding, &operation, None).unwrap(),
        operation
    );
    assert_eq!(store.get_binding(&binding.binding_id).unwrap(), binding);
    assert_eq!(store.claim_next().unwrap().unwrap().binding, binding);
}

#[test]
fn deleted_policy_revisions_leave_gaps_and_are_never_reused() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let pap = PapService::new(Arc::clone(&store));
    let first = pap
        .put_policy(
            None,
            "revision-one",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/one/**".to_owned()],
            },
        )
        .unwrap();
    let second = pap
        .put_policy(
            Some(&first.policy_id),
            "revision-two",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/two/**".to_owned()],
            },
        )
        .unwrap();
    let third = pap
        .put_policy(
            Some(&first.policy_id),
            "revision-three",
            &PolicyTemplate::PreventFileDeletion {
                files: vec!["/three/**".to_owned()],
            },
        )
        .unwrap();

    assert_eq!(second.revision, revision(2));
    assert_eq!(third.revision, revision(3));
    assert_eq!(
        store
            .delete_policy_revision(&first.policy_id, revision(2))
            .unwrap(),
        second
    );
    assert_eq!(store.list_policies(100, 0, usize::MAX).unwrap().total, 2);
    assert_eq!(
        store.get_policy(&first.policy_id, revision(1)).unwrap(),
        first
    );
    assert!(matches!(
        store.get_policy(&first.policy_id, revision(2)),
        Err(PapError::NotFound)
    ));
    assert_eq!(
        store.get_policy(&third.policy_id, revision(3)).unwrap(),
        third
    );

    let fourth = pap
        .put_policy(
            Some(&third.policy_id),
            "revision-four",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/four/**".to_owned()],
            },
        )
        .unwrap();
    assert_eq!(fourth.revision, revision(4));
    let state = store
        .get_policy_revision_state(&fourth.policy_id)
        .unwrap()
        .unwrap();
    assert_eq!(state.last_allocated_revision, revision(4));
    assert_eq!(state.latest, Some(fourth));
}

#[test]
fn binding_identity_revisions_and_operations_are_daemon_owned() {
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let (policy_id, scope_id) = prepare(&store);
    let runtime = PolicyRuntime::new(
        Arc::clone(&store),
        Arc::clone(&store),
        Arc::new(UnavailablePolicyAdapter),
    );
    let first = runtime
        .put_binding(None, &policy_id, revision(1), &scope_id, revision(1))
        .unwrap();
    uuid::Uuid::parse_str(first.binding_id.as_str()).unwrap();
    uuid::Uuid::parse_str(first.operation_id.as_str()).unwrap();
    assert_eq!(first.binding_revision, revision(1));

    let updated = runtime
        .put_binding(
            Some(&first.binding_id),
            &policy_id,
            revision(1),
            &scope_id,
            revision(1),
        )
        .unwrap();
    assert_eq!(updated.binding_id, first.binding_id);
    assert_eq!(updated.binding_revision, revision(2));
    assert_ne!(updated.operation_id, first.operation_id);

    let unknown = id("6efed5ea-47c9-4b14-8e86-888f2ad88fc7");
    assert!(matches!(
        runtime.put_binding(
            Some(&unknown),
            &policy_id,
            revision(1),
            &scope_id,
            revision(1),
        ),
        Err(RuntimeError::NotFound)
    ));
}

#[test]
fn dispatching_operation_is_requeued_after_store_restart() {
    let base = std::env::temp_dir().join(format!("asc-policy-recovery-{}", uuid::Uuid::new_v4()));
    let store = Arc::new(SqlitePolicyStore::open(&base).unwrap());
    let (policy_id, scope_id) = prepare(&store);
    let runtime = PolicyRuntime::new(
        Arc::clone(&store),
        Arc::clone(&store),
        Arc::new(UnavailablePolicyAdapter),
    );
    let accepted = runtime
        .put_binding(None, &policy_id, revision(1), &scope_id, revision(1))
        .unwrap();
    assert!(store.claim_next().unwrap().is_some());
    drop(runtime);
    drop(store);

    let reopened = Arc::new(SqlitePolicyStore::open(&base).unwrap());
    assert_eq!(
        reopened
            .get_operation(&accepted.operation_id)
            .unwrap()
            .state,
        OperationState::Queued
    );
    let fake = Arc::new(FakePolicyAdapter::default());
    let recovered = PolicyRuntime::new(Arc::clone(&reopened), Arc::clone(&reopened), fake);
    assert_eq!(
        recovered.dispatch_once().unwrap().unwrap().state,
        OperationState::Dispatched
    );
    drop(recovered);
    drop(reopened);
    let mut wal = base.as_os_str().to_os_string();
    wal.push("-wal");
    let mut shared_memory = base.as_os_str().to_os_string();
    shared_memory.push("-shm");
    for path in [base, wal.into(), shared_memory.into()] {
        let _ = std::fs::remove_file(path);
    }
}
