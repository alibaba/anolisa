use cosh_gateway_contracts::common::{
    BoundedName, ContractHeader, ContractSchema, Correlation, RuntimeSelector,
};
use cosh_gateway_contracts::external::{ExternalRef, ExternalRefKind};
use cosh_gateway_contracts::ids::{AgentSessionId, InstallationId, MessageId};
use cosh_gateway_contracts::task::{TaskEvent, TaskEventEnvelope};

use super::*;
use crate::storage::{CommitOutcome, TaskCommit};

fn digest(byte: char) -> Digest {
    Digest::parse(byte.to_string().repeat(64)).unwrap()
}

fn target() -> TargetRef {
    TargetRef {
        kind: BoundedName::new("local").unwrap(),
        authority: BoundedName::new("test").unwrap(),
        identifier: BoundedOpaque::new("host").unwrap(),
    }
}

fn command(actor_id: &ActorId, key: &str, byte: char, now_ms: u64) -> LedgerCommand {
    LedgerCommand {
        actor_id: actor_id.clone(),
        idempotency_key: IdempotencyKey::new(key).unwrap(),
        command_digest: digest(byte),
        committed_at_ms: now_ms,
    }
}

fn create_task(store: &mut SqliteTaskStore, actor_id: &ActorId, run_id: &RunId) -> TaskId {
    let task_id = TaskId::new();
    let mut correlation = Correlation::new(InstallationId::new());
    correlation.actor_id = Some(actor_id.clone());
    correlation.task_id = Some(task_id.clone());
    let envelope = |revision, event| TaskEventEnvelope {
        header: ContractHeader::new(
            ContractSchema::TaskEvent,
            MessageId::new(),
            1,
            correlation.clone(),
        ),
        task_id: task_id.clone(),
        revision,
        event,
    };
    let events = vec![
        envelope(
            1,
            TaskEvent::TaskSubmitted {
                intent_digest: digest('0'),
                target: target(),
            },
        ),
        envelope(
            2,
            TaskEvent::TaskQueued {
                run_id: run_id.clone(),
                runtime: RuntimeSelector {
                    runtime: BoundedName::new("acp").unwrap(),
                    profile: Some(BoundedName::new("test").unwrap()),
                },
            },
        ),
        envelope(
            3,
            TaskEvent::RunStarted {
                run_id: run_id.clone(),
            },
        ),
    ];
    let outcome = store
        .commit_task(&TaskCommit {
            actor_id: actor_id.clone(),
            idempotency_key: IdempotencyKey::new(format!("task-{}", task_id.as_str())).unwrap(),
            command_digest: digest('1'),
            expected_revision: Some(0),
            events,
            outbox: Vec::new(),
            committed_at_ms: 1,
        })
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Applied(_)));
    task_id
}

fn acquire_lease(
    store: &mut SqliteTaskStore,
    actor_id: &ActorId,
    task_id: &TaskId,
    run_id: &RunId,
    key: &str,
    now_ms: u64,
    expires_at_ms: u64,
) -> LeaseClaim {
    let lease = LeaseCommand {
        command: command(actor_id, key, 'a', now_ms),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        lease_owner: BoundedOpaque::new(format!("owner-{key}")).unwrap(),
        expires_at_ms,
    };
    let LedgerOutcome::Applied(record) = store.acquire_run_lease(&lease).unwrap() else {
        panic!("lease must apply")
    };
    LeaseClaim {
        task_id: record.task_id,
        run_id: record.run_id,
        lease_owner: record.lease_owner,
        generation: record.generation,
        revision: record.revision,
    }
}

fn approval(actor_id: &ActorId, task_id: &TaskId, run_id: &RunId) -> ApprovalRecord {
    ApprovalRecord {
        approval_id: ApprovalId::new(),
        request_id: RequestId::new(),
        actor_id: actor_id.clone(),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        target: target(),
        operation_digest: digest('2'),
        input_digest: digest('3'),
        state: ApprovalState::Pending,
        revision: 1,
        expires_at_ms: 100,
        decided_by_actor_id: None,
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}

fn approved_fixture(store: &mut SqliteTaskStore) -> (ActorId, TaskId, RunId, ApprovalRecord) {
    let actor_id = ActorId::new();
    let run_id = RunId::new();
    let task_id = create_task(store, &actor_id, &run_id);
    let approval = approval(&actor_id, &task_id, &run_id);
    store
        .create_approval(&command(&actor_id, "create-approval", '4', 10), &approval)
        .unwrap();
    let resolved = store
        .resolve_approval(
            &command(&actor_id, "resolve-approval", '5', 20),
            &approval.approval_id,
            1,
            ApprovalResolution::Decide(ApprovalDecision::Approve),
        )
        .unwrap();
    let LedgerOutcome::Applied(approval) = resolved else {
        panic!("approval must be applied")
    };
    (actor_id, task_id, run_id, approval)
}

fn permit(approval: &ApprovalRecord) -> ExecutionPermit {
    ExecutionPermit {
        permit_id: PermitId::new(),
        request_id: approval.request_id.clone(),
        actor_id: approval.actor_id.clone(),
        approval_id: Some(approval.approval_id.clone()),
        task_id: approval.task_id.clone(),
        run_id: approval.run_id.clone(),
        execution_id: ExecutionId::new(),
        target: approval.target.clone(),
        operation_digest: approval.operation_digest.clone(),
        input_digest: approval.input_digest.clone(),
        policy_revision: 7,
        valid_until_ms: 90,
        single_use: true,
    }
}

fn claim(permit: &ExecutionPermit, lease: &LeaseClaim) -> ExecutionClaim {
    ExecutionClaim {
        permit_id: permit.permit_id.clone(),
        execution_id: permit.execution_id.clone(),
        task_id: permit.task_id.clone(),
        run_id: permit.run_id.clone(),
        target: permit.target.clone(),
        operation_digest: permit.operation_digest.clone(),
        input_digest: permit.input_digest.clone(),
        policy_revision: permit.policy_revision,
        lease: lease.clone(),
    }
}

#[test]
fn approval_resolution_is_actor_revision_deadline_and_idempotency_bound() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let actor_id = ActorId::new();
    let run_id = RunId::new();
    let task_id = create_task(&mut store, &actor_id, &run_id);
    let approval = approval(&actor_id, &task_id, &run_id);
    let create = command(&actor_id, "create", '6', 10);

    assert!(matches!(
        store.create_approval(&create, &approval).unwrap(),
        LedgerOutcome::Applied(_)
    ));
    assert!(matches!(
        store.create_approval(&create, &approval).unwrap(),
        LedgerOutcome::Replayed(_)
    ));
    let attacker = ActorId::new();
    assert!(matches!(
        store.resolve_approval(
            &command(&attacker, "attack", '7', 20),
            &approval.approval_id,
            1,
            ApprovalResolution::Decide(ApprovalDecision::Approve)
        ),
        Err(StoreError::LedgerConflict { .. })
    ));
    let expired = store
        .resolve_approval(
            &command(&actor_id, "late", '8', 100),
            &approval.approval_id,
            1,
            ApprovalResolution::Decide(ApprovalDecision::Approve),
        )
        .unwrap();
    let LedgerOutcome::Applied(expired) = expired else {
        panic!("deadline transition must be applied")
    };
    assert_eq!(expired.state, ApprovalState::Expired);
    assert!(expired.decided_by_actor_id.is_none());
}

#[test]
fn permit_consumption_and_execution_start_are_atomic_and_exactly_bound() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let (actor_id, task_id, run_id, approval) = approved_fixture(&mut store);
    let lease = acquire_lease(
        &mut store,
        &actor_id,
        &task_id,
        &run_id,
        "lease-consume",
        25,
        80,
    );
    let permit = permit(&approval);
    store
        .issue_permit(&command(&actor_id, "issue", '9', 30), &permit)
        .unwrap();

    let exact = claim(&permit, &lease);
    let mut substitutions = Vec::new();
    let mut task = exact.clone();
    task.task_id = TaskId::new();
    substitutions.push(task);
    let mut run = exact.clone();
    run.run_id = RunId::new();
    substitutions.push(run);
    let mut changed_target = exact.clone();
    changed_target.target = TargetRef {
        kind: BoundedName::new("local").unwrap(),
        authority: BoundedName::new("test").unwrap(),
        identifier: BoundedOpaque::new("other-host").unwrap(),
    };
    substitutions.push(changed_target);
    let mut operation = exact.clone();
    operation.operation_digest = digest('a');
    substitutions.push(operation);
    let mut input = exact.clone();
    input.input_digest = digest('b');
    substitutions.push(input);
    let mut execution = exact.clone();
    execution.execution_id = ExecutionId::new();
    substitutions.push(execution);
    for (index, substituted) in substitutions.iter().enumerate() {
        let result = store.consume_permit_and_start_execution(
            &command(
                &actor_id,
                &format!("substitute-{index}"),
                char::from_digit(u32::try_from(index + 1).unwrap(), 10).unwrap(),
                40,
            ),
            substituted,
        );
        assert!(result.is_err(), "substitution {index} must fail closed");
    }
    let attacker = ActorId::new();
    assert!(store
        .consume_permit_and_start_execution(
            &command(&attacker, "substitute-actor", '7', 40),
            &exact,
        )
        .is_err());
    let permit_state: String = store
        .connection()
        .query_row(
            "SELECT state FROM permits WHERE permit_id=?1",
            params![permit.permit_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let execution_state: String = store
        .connection()
        .query_row(
            "SELECT state FROM executions WHERE execution_id=?1",
            params![permit.execution_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        (permit_state.as_str(), execution_state.as_str()),
        ("issued", "planned")
    );

    let started = store
        .consume_permit_and_start_execution(&command(&actor_id, "consume", 'b', 40), &exact)
        .unwrap();
    let LedgerOutcome::Applied(started) = started else {
        panic!("consumption must apply")
    };
    assert_eq!(started.state, ExecutionState::Started);
    assert_eq!(started.revision, 2);
    assert!(matches!(
        store.consume_permit_and_start_execution(
            &command(&actor_id, "reuse", 'c', 41),
            &claim(&permit, &lease)
        ),
        Err(StoreError::LedgerConflict { .. })
    ));
}

#[test]
fn completion_is_revisioned_and_persists_one_evidence_receipt() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let (actor_id, task_id, run_id, approval) = approved_fixture(&mut store);
    let lease = acquire_lease(
        &mut store,
        &actor_id,
        &task_id,
        &run_id,
        "lease-complete",
        25,
        80,
    );
    let permit = permit(&approval);
    store
        .issue_permit(&command(&actor_id, "issue", 'd', 30), &permit)
        .unwrap();
    store
        .consume_permit_and_start_execution(
            &command(&actor_id, "consume", 'e', 40),
            &claim(&permit, &lease),
        )
        .unwrap();
    let completion = ExecutionCompletion {
        execution_id: permit.execution_id.clone(),
        expected_revision: 2,
        succeeded: true,
        receipt_digest: digest('f'),
        safe_detail: Some(BoundedText::new("completed").unwrap()),
    };
    let completed = store
        .complete_execution(&command(&actor_id, "complete", 'f', 50), &completion)
        .unwrap();
    let LedgerOutcome::Applied(completed) = completed else {
        panic!("must apply")
    };
    assert_eq!(completed.state, ExecutionState::Succeeded);
    let receipt_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM execution_receipts WHERE execution_id=?1",
            params![permit.execution_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt_count, 1);
}

#[test]
fn recovery_marks_started_execution_uncertain_without_retry() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let (actor_id, task_id, run_id, approval) = approved_fixture(&mut store);
    let lease = acquire_lease(&mut store, &actor_id, &task_id, &run_id, "recover", 25, 80);
    let permit = permit(&approval);
    store
        .issue_permit(&command(&actor_id, "issue", '1', 30), &permit)
        .unwrap();
    store
        .consume_permit_and_start_execution(
            &command(&actor_id, "consume", '2', 40),
            &claim(&permit, &lease),
        )
        .unwrap();

    let report = store.recover_gateway(60).unwrap();
    assert_eq!(report.executions_uncertain, 1);
    let state: String = store
        .connection()
        .query_row(
            "SELECT state FROM executions WHERE execution_id=?1",
            params![permit.execution_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "uncertain");
    let receipts: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM execution_receipts WHERE execution_id=?1",
            params![permit.execution_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 0);
}

fn runtime_binding(task_id: &TaskId, run_id: &RunId, generation: u64) -> RuntimeBindingRef {
    RuntimeBindingRef {
        binding_id: RuntimeBindingId::new(),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        agent_session_id: AgentSessionId::new(),
        runtime_instance_id: RuntimeInstanceId::new(),
        runtime_generation: generation,
        external_session: ExternalRef {
            kind: ExternalRefKind::AcpSession,
            authority: BoundedName::new("test").unwrap(),
            scope_digest: digest('3'),
            value: BoundedOpaque::new("session").unwrap(),
        },
    }
}

#[test]
fn runtime_generation_and_sequence_fence_stale_output() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let actor_id = ActorId::new();
    let run_id = RunId::new();
    let task_id = create_task(&mut store, &actor_id, &run_id);
    let lease = acquire_lease(&mut store, &actor_id, &task_id, &run_id, "runtime", 5, 100);
    let first = runtime_binding(&task_id, &run_id, 1);
    store
        .bind_runtime(&command(&actor_id, "bind-1", '4', 10), &first, &lease)
        .unwrap();
    store
        .record_runtime_sequence(
            &first.binding_id,
            &first.runtime_instance_id,
            1,
            1,
            11,
            &lease,
        )
        .unwrap();
    assert!(matches!(
        store.record_runtime_sequence(
            &first.binding_id,
            &first.runtime_instance_id,
            1,
            1,
            12,
            &lease,
        ),
        Err(StoreError::LedgerConflict { .. })
    ));
    let second = runtime_binding(&task_id, &run_id, 2);
    store
        .bind_runtime(&command(&actor_id, "bind-2", '5', 20), &second, &lease)
        .unwrap();
    assert!(matches!(
        store.record_runtime_sequence(
            &first.binding_id,
            &first.runtime_instance_id,
            1,
            2,
            21,
            &lease,
        ),
        Err(StoreError::LedgerConflict { .. })
    ));
    let stale_generation = runtime_binding(&task_id, &run_id, 1);
    assert!(matches!(
        store.bind_runtime(
            &command(&actor_id, "stale", '6', 21),
            &stale_generation,
            &lease,
        ),
        Err(StoreError::GenerationFenced { .. })
    ));
}

#[test]
fn expired_run_lease_takeover_increments_fencing_generation() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let actor_id = ActorId::new();
    let run_id = RunId::new();
    let task_id = create_task(&mut store, &actor_id, &run_id);
    let first = LeaseCommand {
        command: command(&actor_id, "lease-1", '7', 10),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        lease_owner: BoundedOpaque::new("coordinator-a").unwrap(),
        expires_at_ms: 20,
    };
    let LedgerOutcome::Applied(first_record) = store.acquire_run_lease(&first).unwrap() else {
        panic!("first lease must apply")
    };
    assert_eq!(first_record.generation, 1);
    let renewal = LeaseCommand {
        command: command(&actor_id, "lease-renew", 'a', 12),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        lease_owner: BoundedOpaque::new("coordinator-a").unwrap(),
        expires_at_ms: 22,
    };
    let LedgerOutcome::Applied(renewed) = store
        .renew_run_lease(&renewal, first_record.generation, first_record.revision)
        .unwrap()
    else {
        panic!("renewal must apply")
    };
    assert_eq!(renewed.generation, 1);
    assert_eq!(renewed.revision, 2);
    let held = LeaseCommand {
        command: command(&actor_id, "lease-held", '8', 21),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        lease_owner: BoundedOpaque::new("coordinator-b").unwrap(),
        expires_at_ms: 30,
    };
    assert!(matches!(
        store.acquire_run_lease(&held),
        Err(StoreError::LedgerConflict { .. })
    ));
    let takeover = LeaseCommand {
        command: command(&actor_id, "lease-2", '9', 22),
        task_id,
        run_id,
        lease_owner: BoundedOpaque::new("coordinator-b").unwrap(),
        expires_at_ms: 30,
    };
    let LedgerOutcome::Applied(second_record) = store.acquire_run_lease(&takeover).unwrap() else {
        panic!("takeover must apply")
    };
    assert_eq!(second_record.generation, 2);
    assert_eq!(second_record.revision, 3);
    assert_eq!(
        store.load_run_lease(&second_record.run_id).unwrap(),
        second_record
    );
}

#[test]
fn released_run_lease_can_be_reacquired_only_with_a_new_generation() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let actor_id = ActorId::new();
    let run_id = RunId::new();
    let task_id = create_task(&mut store, &actor_id, &run_id);
    let acquired = LeaseCommand {
        command: command(&actor_id, "lease-acquire", 'b', 10),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        lease_owner: BoundedOpaque::new("coordinator-a").unwrap(),
        expires_at_ms: 30,
    };
    let LedgerOutcome::Applied(first) = store.acquire_run_lease(&acquired).unwrap() else {
        panic!("lease must apply")
    };
    let released = store
        .release_run_lease(
            &command(&actor_id, "lease-release", 'c', 15),
            &LeaseClaim {
                task_id: task_id.clone(),
                run_id: run_id.clone(),
                lease_owner: first.lease_owner,
                generation: first.generation,
                revision: first.revision,
            },
        )
        .unwrap();
    let LedgerOutcome::Applied(released) = released else {
        panic!("release must apply")
    };
    assert_eq!(released.expires_at_ms, 15);
    let reacquire = LeaseCommand {
        command: command(&actor_id, "lease-reacquire", 'd', 15),
        task_id,
        run_id,
        lease_owner: BoundedOpaque::new("coordinator-b").unwrap(),
        expires_at_ms: 40,
    };
    let LedgerOutcome::Applied(second) = store.acquire_run_lease(&reacquire).unwrap() else {
        panic!("reacquire must apply")
    };
    assert_eq!(second.generation, 2);
}

#[test]
fn stale_lease_and_cross_task_run_claims_roll_back_atomically() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let (actor_id, task_id, run_id, approval) = approved_fixture(&mut store);
    let stale = acquire_lease(
        &mut store,
        &actor_id,
        &task_id,
        &run_id,
        "lease-stale",
        25,
        70,
    );
    let renewal = LeaseCommand {
        command: command(&actor_id, "renew-stale", '2', 30),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        lease_owner: stale.lease_owner.clone(),
        expires_at_ms: 80,
    };
    store
        .renew_run_lease(&renewal, stale.generation, stale.revision)
        .unwrap();
    let permit = permit(&approval);
    store
        .issue_permit(&command(&actor_id, "issue-stale", '3', 35), &permit)
        .unwrap();

    assert!(matches!(
        store.consume_permit_and_start_execution(
            &command(&actor_id, "consume-stale", '4', 40),
            &claim(&permit, &stale),
        ),
        Err(StoreError::LedgerConflict { .. })
    ));

    let other_run = RunId::new();
    let _other_task = create_task(&mut store, &actor_id, &other_run);
    let mut substituted = claim(&permit, &stale);
    substituted.run_id = other_run;
    assert!(matches!(
        store.consume_permit_and_start_execution(
            &command(&actor_id, "consume-other-run", '5', 40),
            &substituted,
        ),
        Err(StoreError::LedgerConflict { .. })
    ));

    let states = store
        .connection()
        .query_row(
            "SELECT p.state, e.state FROM permits p JOIN executions e
             ON e.execution_id=p.execution_id WHERE p.permit_id=?1",
            params![permit.permit_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(states, ("issued".to_owned(), "planned".to_owned()));
}

#[test]
fn permit_deadline_cannot_outlive_approval_and_overflow_rolls_back() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let (actor_id, task_id, run_id, approved) = approved_fixture(&mut store);
    let mut widened = permit(&approved);
    widened.valid_until_ms = approved.expires_at_ms + 1;
    assert!(matches!(
        store.issue_permit(&command(&actor_id, "issue-wide", '6', 30), &widened),
        Err(StoreError::LedgerConflict { .. })
    ));

    let mut overflow = approval(&actor_id, &task_id, &run_id);
    overflow.approval_id = ApprovalId::new();
    overflow.request_id = RequestId::new();
    overflow.expires_at_ms = u64::MAX;
    assert!(matches!(
        store.create_approval(&command(&actor_id, "create-overflow", '7', 40), &overflow,),
        Err(StoreError::LedgerConflict { .. })
    ));

    let (permits, executions, approvals): (i64, i64, i64) = store
        .connection()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM permits),
                    (SELECT COUNT(*) FROM executions),
                    (SELECT COUNT(*) FROM approvals WHERE approval_id=?1)",
            params![overflow.approval_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((permits, executions, approvals), (0, 0, 0));
}

#[test]
fn runtime_acceptance_requires_current_lease_and_exact_next_generation() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let actor_id = ActorId::new();
    let run_id = RunId::new();
    let task_id = create_task(&mut store, &actor_id, &run_id);
    let stale = acquire_lease(
        &mut store,
        &actor_id,
        &task_id,
        &run_id,
        "runtime-stale",
        5,
        30,
    );
    let first = runtime_binding(&task_id, &run_id, 1);
    store
        .bind_runtime(&command(&actor_id, "bind-first", '8', 10), &first, &stale)
        .unwrap();
    let skipped = runtime_binding(&task_id, &run_id, 3);
    assert!(matches!(
        store.bind_runtime(&command(&actor_id, "bind-skip", '9', 11), &skipped, &stale,),
        Err(StoreError::GenerationFenced {
            expected: 2,
            actual: 3
        })
    ));

    let renewal = LeaseCommand {
        command: command(&actor_id, "runtime-renew", 'a', 12),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        lease_owner: stale.lease_owner.clone(),
        expires_at_ms: 40,
    };
    store
        .renew_run_lease(&renewal, stale.generation, stale.revision)
        .unwrap();
    assert!(matches!(
        store.record_runtime_sequence(
            &first.binding_id,
            &first.runtime_instance_id,
            1,
            1,
            13,
            &stale,
        ),
        Err(StoreError::LedgerConflict { .. })
    ));
    assert_eq!(
        store
            .load_runtime_binding_record(&first.binding_id)
            .unwrap()
            .last_sequence,
        0
    );
}

#[test]
fn terminal_receipt_corruption_fails_load_and_recovery_without_mutation() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let (actor_id, task_id, run_id, approval) = approved_fixture(&mut store);
    let lease = acquire_lease(
        &mut store,
        &actor_id,
        &task_id,
        &run_id,
        "receipt-corrupt",
        25,
        80,
    );
    let permit = permit(&approval);
    store
        .issue_permit(&command(&actor_id, "issue-receipt", 'b', 30), &permit)
        .unwrap();
    store
        .consume_permit_and_start_execution(
            &command(&actor_id, "consume-receipt", 'c', 40),
            &claim(&permit, &lease),
        )
        .unwrap();
    store
        .complete_execution(
            &command(&actor_id, "complete-receipt", 'd', 50),
            &ExecutionCompletion {
                execution_id: permit.execution_id.clone(),
                expected_revision: 2,
                succeeded: true,
                receipt_digest: digest('e'),
                safe_detail: None,
            },
        )
        .unwrap();
    store
        .connection()
        .execute(
            "UPDATE execution_receipts SET state='failed' WHERE execution_id=?1",
            params![permit.execution_id.as_str()],
        )
        .unwrap();

    assert!(matches!(
        store.load_execution_record(&permit.execution_id),
        Err(StoreError::Corrupt { .. })
    ));
    assert!(matches!(
        store.recover_gateway(60),
        Err(StoreError::Corrupt { .. })
    ));
    let state: String = store
        .connection()
        .query_row(
            "SELECT state FROM executions WHERE execution_id=?1",
            params![permit.execution_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "succeeded");
}

#[test]
fn task_and_ledger_commands_share_one_idempotency_namespace() {
    let mut store = SqliteTaskStore::open_in_memory().unwrap();
    let actor_id = ActorId::new();
    let run_id = RunId::new();
    let task_id = create_task(&mut store, &actor_id, &run_id);
    let approval = approval(&actor_id, &task_id, &run_id);
    let task_key = format!("task-{}", task_id.as_str());
    let result = store.create_approval(&command(&actor_id, &task_key, 'f', 10), &approval);
    assert!(matches!(result, Err(StoreError::IdempotencyConflict)));
    let count: i64 = store
        .connection()
        .query_row("SELECT COUNT(*) FROM approvals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
