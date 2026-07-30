use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agentsight::enforcement::{EnforcementClient, EnforcementCoordinator, EnforcementStore};
use agentsight_enforcement_protocol::{ApplyPolicy, BindingState, Effect, ViolationEvent};
use agentsight_enforcer::{EnforcerService, MockBackend};
use uuid::Uuid;

struct TestEnforcer {
    socket_path: PathBuf,
    database_path: PathBuf,
    backend: Arc<MockBackend>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl TestEnforcer {
    fn start() -> Self {
        let id = Uuid::new_v4();
        let socket_path = PathBuf::from(format!("/tmp/agentsight-pipeline-{id}.sock"));
        let database_path = PathBuf::from(format!("/tmp/agentsight-enforcement-{id}.db"));
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
        wait_for_path(&socket_path);
        Self {
            socket_path,
            database_path,
            backend,
            stop,
            worker: Some(worker),
        }
    }

    fn apply_request(&self) -> ApplyPolicy {
        ApplyPolicy {
            binding_id: Uuid::new_v4(),
            agent_id: "pipeline-agent".into(),
            session_id: Some("pipeline-session".into()),
            root_pid: 4242,
            process_start_time: 99,
            policy_id: "pipeline-policy".into(),
            policy_revision: "revision-1".into(),
            policy_dsl: "label AGENT".into(),
        }
    }

    fn violation(&self, request: &ApplyPolicy) -> ViolationEvent {
        ViolationEvent {
            event_id: Uuid::new_v4(),
            binding_id: request.binding_id,
            agent_id: request.agent_id.clone(),
            session_id: request.session_id.clone(),
            policy_id: request.policy_id.clone(),
            policy_revision: request.policy_revision.clone(),
            pid: request.root_pid,
            ppid: Some(1),
            process_start_time: request.process_start_time,
            operation: "open".into(),
            target: "/tmp/secret".into(),
            effect: Effect::Block,
            blocked: true,
            killed: false,
            rule_id: Some("block-secret".into()),
            reason: Some("pipeline fixture".into()),
            occurred_at_ns: 100,
            observed_at_ns: 101,
            actplane_revision: "mock".into(),
        }
    }
}

impl Drop for TestEnforcer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("fixture enforcer should stop");
        }
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.database_path);
        let _ = fs::remove_file(format!("{}-wal", self.database_path.display()));
        let _ = fs::remove_file(format!("{}-shm", self.database_path.display()));
    }
}

fn wait_for_path(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("fixture path did not appear");
}

#[test]
fn apply_persists_enforced_state_and_deduplicates_violation() {
    let fixture = TestEnforcer::start();
    let store = EnforcementStore::open(&fixture.database_path)
        .expect("temporary enforcement store should open");
    let coordinator =
        EnforcementCoordinator::new(EnforcementClient::new(&fixture.socket_path), store);
    let request = fixture.apply_request();
    let binding = coordinator
        .apply(request.clone())
        .expect("mock apply should work");
    assert_eq!(binding.state, BindingState::Enforced);

    let ingestion = coordinator
        .start_ingestion()
        .expect("ingestion should start");
    thread::sleep(Duration::from_millis(50));
    let violation = fixture.violation(&request);
    fixture
        .backend
        .publish_violation(violation.clone())
        .expect("first violation should publish");
    fixture
        .backend
        .publish_violation(violation)
        .expect("duplicate violation should publish");

    for _ in 0..100 {
        if coordinator
            .violations(100)
            .expect("violation query should work")
            .len()
            == 1
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        coordinator
            .violations(100)
            .expect("violation query should work")
            .len(),
        1
    );
    coordinator.stop_ingestion();
    ingestion.join().expect("ingestion should stop");
}
