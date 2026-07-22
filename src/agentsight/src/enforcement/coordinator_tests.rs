use super::*;

fn apply_request() -> ApplyPolicy {
    ApplyPolicy {
        binding_id: Uuid::new_v4(),
        agent_id: "reconciliation-agent".into(),
        session_id: Some("reconciliation-session".into()),
        root_pid: 42,
        process_start_time: 99,
        policy_id: "reconciliation-policy".into(),
        policy_revision: "revision-1".into(),
        policy_dsl: "label AGENT".into(),
    }
}

fn enforced_binding(request: ApplyPolicy, domain_id: u32) -> Binding {
    Binding {
        request,
        state: BindingState::Enforced,
        message: None,
        domain_id: Some(domain_id),
    }
}

#[test]
fn remote_rejection_fails_one_binding_and_allows_following_binding() {
    let store = EnforcementStore::open(":memory:").expect("test store should open");
    let rejected_request = apply_request();
    let accepted_request = apply_request();

    super::reconciliation::persist_reconciled_apply(
        &store,
        rejected_request.clone(),
        Err(EnforcementError::Remote {
            code: "kernel_failure".into(),
            message: "target process is no longer alive".into(),
        }),
    )
    .expect("remote rejection should not abort reconciliation");
    super::reconciliation::persist_reconciled_apply(
        &store,
        accepted_request.clone(),
        Ok(enforced_binding(accepted_request.clone(), 7)),
    )
    .expect("following binding should still reconcile");

    let rejected = store
        .binding(rejected_request.binding_id)
        .expect("rejected binding should load")
        .expect("rejected binding should remain persisted");
    assert_eq!(rejected.state, BindingState::Failed);
    assert_eq!(
        rejected.message.as_deref(),
        Some("enforcer rejected request (kernel_failure): target process is no longer alive")
    );
    assert_eq!(rejected.domain_id, None);
    assert_eq!(
        store
            .binding(accepted_request.binding_id)
            .expect("accepted binding should load"),
        Some(enforced_binding(accepted_request, 7))
    );
}

#[test]
fn non_remote_apply_error_still_aborts_reconciliation() {
    let store = EnforcementStore::open(":memory:").expect("test store should open");
    let request = apply_request();

    let result = super::reconciliation::persist_reconciled_apply(
        &store,
        request.clone(),
        Err(EnforcementError::Io(std::io::Error::other(
            "fixture transport failure",
        ))),
    );

    assert!(matches!(
        result,
        Err(EnforcementCoordinatorError::Client(EnforcementError::Io(_)))
    ));
    assert_eq!(
        store
            .binding(request.binding_id)
            .expect("binding query should work"),
        None
    );
}

#[test]
fn failed_start_does_not_supersede_ready_worker() {
    let coordinator = EnforcementCoordinator::new(
        EnforcementClient::new("/tmp/unused-enforcement.sock"),
        EnforcementStore::open(":memory:").expect("test store should open"),
    );
    let active = coordinator.ingestion_readiness.candidate();
    coordinator.ingestion_readiness.install(Arc::clone(&active));
    assert!(coordinator.ingestion_readiness.mark_ready(&active));

    let result = coordinator
        .start_ingestion_with(|_| Err(std::io::Error::other("fixture thread spawn failure")));

    assert!(matches!(
        result,
        Err(EnforcementCoordinatorError::Thread(_))
    ));
    assert!(coordinator.ingestion_readiness.is_current(&active));
    assert!(coordinator.ingestion_readiness.is_ready());
}

#[test]
fn ancient_worker_never_becomes_current_after_replacements() {
    let readiness = IngestionReadiness::new(INGESTION_UNAVAILABLE_MESSAGE);
    let ancient = readiness.candidate();
    readiness.install(Arc::clone(&ancient));
    assert!(readiness.mark_ready(&ancient));

    for _ in 0..10_000 {
        let current = readiness.candidate();
        readiness.install(Arc::clone(&current));
        assert!(!readiness.is_current(&ancient));
        assert!(!readiness.mark_ready(&ancient));
        assert!(readiness.mark_ready(&current));
    }
}

#[test]
fn superseded_worker_cannot_publish_or_revoke_current_readiness() {
    let readiness = IngestionReadiness::new(INGESTION_UNAVAILABLE_MESSAGE);
    let first = readiness.candidate();
    readiness.install(Arc::clone(&first));
    assert!(readiness.mark_ready(&first));

    let second = readiness.candidate();
    readiness.install(Arc::clone(&second));
    assert!(!readiness.is_ready());
    assert!(!readiness.mark_ready(&first));
    assert!(readiness.mark_ready(&second));
    readiness.mark_not_ready(&first);
    assert!(readiness.is_ready());

    readiness.stop();
    assert!(!readiness.mark_ready(&second));
    assert!(!readiness.is_ready());
}

#[test]
fn health_combines_backend_and_ingestion_failures() {
    let readiness = IngestionReadiness::new(INGESTION_UNAVAILABLE_MESSAGE);
    let worker = readiness.candidate();
    readiness.install(Arc::clone(&worker));
    readiness.mark_unavailable(
        &worker,
        "violation persistence failed: database is locked".into(),
    );

    let health = combine_health(
        agentsight_enforcement_protocol::HealthStatus {
            ready: false,
            backend: "actplane".into(),
            message: Some("violation event buffer overflow: dropped_events=1".into()),
        },
        &readiness,
    );

    assert!(!health.ready);
    assert_eq!(
        health.message.as_deref(),
        Some(
            "violation event buffer overflow: dropped_events=1; violation persistence failed: database is locked"
        )
    );
}
