#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentsight::enforcement::{ApplyPolicy, EnforcementClient};
use agentsight::security::{RiskCaseStatus, RiskSeverity, SecurityCoordinator, SecurityStore};
use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, CredentialExfiltrationPolicy, DestinationScope, PolicyMode,
    SecurityEventKind,
};
use agentsight_enforcer::{EnforcementBackend, EnforcerService, MockBackend};
use uuid::Uuid;

fn audit_request(binding_id: Uuid) -> ApplyPolicy {
    ApplyPolicy {
        binding_id,
        agent_id: "hermes-test".into(),
        session_id: Some("session-1".into()),
        root_pid: 4242,
        process_start_time: 99,
        policy_id: "credential-exfiltration".into(),
        policy_revision: "3".into(),
        policy_dsl: "mode=audit".into(),
    }
}

fn credential_request(binding_id: Uuid, mode: PolicyMode) -> ApplyCredentialPolicy {
    ApplyCredentialPolicy {
        binding_id,
        agent_id: "hermes-test".into(),
        session_id: Some("session-1".into()),
        root_pid: 4242,
        process_start_time: 99,
        policy: CredentialExfiltrationPolicy {
            policy_id: "credential-exfiltration".into(),
            revision: 3,
            source_patterns: vec!["/root/.ssh/id_rsa".into()],
            trusted_endpoints: Vec::new(),
            taint_label: "CREDENTIAL".into(),
            taint_ttl_secs: 900,
            destination_scope: DestinationScope::PublicIpv4,
            mode,
        },
    }
}

fn ingest_mock_chain(mode: PolicyMode) -> (Arc<SecurityStore>, Vec<uuid::Uuid>) {
    let backend = MockBackend::new();
    let binding_id = Uuid::new_v4();
    backend
        .apply_credential_policy(credential_request(binding_id, mode))
        .expect("product binding should apply");
    let receiver = backend.subscribe_security_events();
    backend
        .emit_credential_exfiltration(binding_id, "/root/.ssh/id_rsa", "8.8.8.8:443")
        .expect("mock chain should emit");

    let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
    let coordinator = SecurityCoordinator::new(
        EnforcementClient::new("/unused/agentsight-enforcer.sock"),
        Arc::clone(&store),
    );
    let mut event_ids = Vec::new();
    for _ in 0..4 {
        let event = receiver.recv().expect("mock event should arrive");
        event_ids.push(event.event_id);
        coordinator.ingest(event).expect("event should ingest");
    }
    (store, event_ids)
}

#[test]
fn audit_chain_creates_one_open_unblocked_case_with_ordered_evidence() {
    let (store, _) = ingest_mock_chain(PolicyMode::Audit);

    let cases = store.list_cases(100, 0).expect("case list should load");
    assert_eq!(cases.len(), 1);
    let detail = store
        .case_detail(cases[0].case_id)
        .expect("case detail should load");

    assert_eq!(detail.case.status, RiskCaseStatus::Open);
    assert_eq!(detail.case.risk_score, 85);
    assert!(!detail.case.blocked);
    assert_eq!(detail.evidence.len(), 4);
    assert!(matches!(
        detail.evidence[0].kind,
        SecurityEventKind::FileAction(_)
    ));
    assert!(matches!(
        detail.evidence[3].kind,
        SecurityEventKind::PolicyDecision(_)
    ));
}

#[test]
fn repeated_decision_does_not_duplicate_the_case() {
    let (store, event_ids) = ingest_mock_chain(PolicyMode::Audit);
    let decision = store
        .event(*event_ids.last().expect("decision id should exist"))
        .expect("decision query should work")
        .expect("decision should exist");
    let coordinator = SecurityCoordinator::new(
        EnforcementClient::new("/unused/agentsight-enforcer.sock"),
        Arc::clone(&store),
    );

    coordinator
        .ingest(decision)
        .expect("duplicate should ingest");

    assert_eq!(
        store
            .list_cases(100, 0)
            .expect("case list should load")
            .len(),
        1
    );
}

#[test]
fn dns_destination_retries_share_one_case_and_append_evidence() {
    let backend = MockBackend::new();
    let binding_id = Uuid::new_v4();
    let request = audit_request(binding_id);
    backend
        .apply(request)
        .expect("binding should apply for retry fixture");
    let receiver = backend.subscribe_security_events();
    backend
        .emit_credential_exfiltration(binding_id, "/root/.ssh/id_rsa", "198.51.100.10:443")
        .expect("first destination should emit");
    backend
        .emit_credential_exfiltration(binding_id, "/root/.ssh/id_rsa", "198.51.100.11:443")
        .expect("second destination should emit");

    let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
    let coordinator = SecurityCoordinator::new(
        EnforcementClient::new("/unused/agentsight-enforcer.sock"),
        Arc::clone(&store),
    );
    for _ in 0..8 {
        coordinator
            .ingest(receiver.recv().expect("mock event should arrive"))
            .expect("mock event should ingest");
    }

    let cases = store.list_cases(100, 0).expect("case list should load");
    assert_eq!(cases.len(), 1, "DNS fallback must remain one risk case");
    let detail = store
        .case_detail(cases[0].case_id)
        .expect("case detail should load");
    assert_eq!(detail.evidence.len(), 8);
    assert!(matches!(
        detail.evidence[0].kind,
        SecurityEventKind::FileAction(_)
    ));
    assert!(matches!(
        detail.evidence[4].kind,
        SecurityEventKind::FileAction(_)
    ));
}

#[test]
fn product_observe_chain_persists_events_without_opening_a_case() {
    let (store, _) = ingest_mock_chain(PolicyMode::Observe);

    assert_eq!(
        store.summary().expect("summary should load").total_events,
        4
    );
    assert!(
        store
            .list_cases(100, 0)
            .expect("case list should load")
            .is_empty()
    );
}

#[test]
fn enforce_chain_creates_a_critical_blocked_case() {
    let (store, _) = ingest_mock_chain(PolicyMode::Enforce);

    let case = store
        .list_cases(10, 0)
        .expect("case list should load")
        .remove(0);
    assert_eq!(case.severity, RiskSeverity::Critical);
    assert!(case.blocked);
}

#[test]
fn client_receives_normalized_security_events_over_uds() {
    let socket_path = format!("/tmp/agentsight-security-{}.sock", Uuid::new_v4());
    let backend = Arc::new(MockBackend::new());
    let service = EnforcerService::bind(&socket_path, Arc::clone(&backend), None)
        .expect("fixture enforcer should bind");
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        service
            .serve_until(&worker_stop)
            .expect("fixture enforcer should run");
    });
    let client = EnforcementClient::new(&socket_path);
    let request = audit_request(Uuid::new_v4());
    let required_subscription = client.subscribe_required().expect("subscribe required");
    client
        .apply(request.clone(), required_subscription.subscription_id())
        .expect("binding should apply");
    let mut subscription = client
        .subscribe_security_events()
        .expect("security subscription should open");
    backend
        .emit_credential_exfiltration(request.binding_id, "/root/.ssh/id_rsa", "198.51.100.10:443")
        .expect("mock chain should emit");

    let deadline = Instant::now() + Duration::from_secs(2);
    let event = loop {
        if let Some(event) = subscription
            .next_event()
            .expect("stream should remain valid")
        {
            break event;
        }
        assert!(Instant::now() < deadline, "security event timed out");
    };
    assert!(matches!(event.kind, SecurityEventKind::FileAction(_)));

    stop.store(true, Ordering::Release);
    drop(subscription);
    drop(required_subscription);
    worker.join().expect("fixture enforcer should stop");
    let _ = std::fs::remove_file(socket_path);
}

#[test]
fn coordinator_worker_ingests_the_uds_stream() {
    let socket_path = format!("/tmp/agentsight-security-worker-{}.sock", Uuid::new_v4());
    let backend = Arc::new(MockBackend::new());
    let service = EnforcerService::bind(&socket_path, Arc::clone(&backend), None)
        .expect("fixture enforcer should bind");
    let service_stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&service_stop);
    let service_worker = thread::spawn(move || {
        service
            .serve_until(&worker_stop)
            .expect("fixture enforcer should run");
    });
    let client = EnforcementClient::new(&socket_path);
    let store = Arc::new(SecurityStore::open_in_memory().expect("fixture store should open"));
    let coordinator = SecurityCoordinator::new(client.clone(), Arc::clone(&store));
    let coordinator_worker = coordinator.start().expect("coordinator should start");
    let request = audit_request(Uuid::new_v4());
    let required_subscription = client.subscribe_required().expect("subscribe required");
    client
        .apply(request.clone(), required_subscription.subscription_id())
        .expect("binding should apply");
    thread::sleep(Duration::from_millis(100));
    backend
        .emit_credential_exfiltration(request.binding_id, "/root/.ssh/id_rsa", "198.51.100.10:443")
        .expect("mock chain should emit");

    let deadline = Instant::now() + Duration::from_secs(2);
    while store
        .list_cases(10, 0)
        .expect("case list should load")
        .is_empty()
    {
        assert!(Instant::now() < deadline, "risk case timed out");
        thread::sleep(Duration::from_millis(20));
    }

    coordinator.stop();
    coordinator_worker
        .join()
        .expect("coordinator worker should stop");
    drop(required_subscription);
    service_stop.store(true, Ordering::Release);
    service_worker.join().expect("fixture enforcer should stop");
    let _ = std::fs::remove_file(socket_path);
}
