use super::*;

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
