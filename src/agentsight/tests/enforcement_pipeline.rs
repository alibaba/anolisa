use std::fs;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agentsight::enforcement::{
    EnforcementClient, EnforcementCoordinator, EnforcementCoordinatorError, EnforcementStore,
};
use agentsight_enforcement_protocol::{
    ApplyPolicy, Binding, BindingState, Command, Effect, HealthStatus, Request, Response,
    ResponseBody, ViolationEvent, read_frame, write_frame,
};
use agentsight_enforcer::{EnforcerService, MockBackend};
use uuid::Uuid;

struct TestEnforcer {
    socket_path: PathBuf,
    database_path: PathBuf,
    backend: Arc<MockBackend>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubscriptionState {
    AckBlocked,
    Connected,
}

struct ControlledState {
    subscription: SubscriptionState,
    subscribe_attempts: usize,
    bindings: Vec<Binding>,
}

struct ControlledEnforcer {
    socket_path: PathBuf,
    database_path: PathBuf,
    state: Arc<(Mutex<ControlledState>, Condvar)>,
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

impl ControlledEnforcer {
    fn start() -> Self {
        let id = Uuid::new_v4();
        let socket_path = PathBuf::from(format!("/tmp/agentsight-controlled-{id}.sock"));
        let database_path = PathBuf::from(format!("/tmp/agentsight-controlled-{id}.db"));
        let listener = UnixListener::bind(&socket_path).expect("controlled enforcer should bind");
        listener
            .set_nonblocking(true)
            .expect("controlled listener should be nonblocking");
        let state = Arc::new((
            Mutex::new(ControlledState {
                subscription: SubscriptionState::AckBlocked,
                subscribe_attempts: 0,
                bindings: Vec::new(),
            }),
            Condvar::new(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut connections = Vec::new();
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let connection_state = Arc::clone(&worker_state);
                        let connection_stop = Arc::clone(&worker_stop);
                        connections.push(thread::spawn(move || {
                            handle_controlled_connection(stream, connection_state, connection_stop);
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("controlled enforcer accept failed: {error}"),
                }
            }
            for connection in connections {
                connection
                    .join()
                    .expect("controlled connection should stop");
            }
        });
        Self {
            socket_path,
            database_path,
            state,
            stop,
            worker: Some(worker),
        }
    }

    fn apply_request(&self) -> ApplyPolicy {
        ApplyPolicy {
            binding_id: Uuid::new_v4(),
            agent_id: "controlled-agent".into(),
            session_id: Some("controlled-session".into()),
            root_pid: 4242,
            process_start_time: 99,
            policy_id: "controlled-policy".into(),
            policy_revision: "revision-1".into(),
            policy_dsl: "label AGENT".into(),
        }
    }

    fn wait_for_subscribe_attempt(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        let (state, changed) = &*self.state;
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.subscribe_attempts == 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, timeout) = changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if timeout.timed_out() && state.subscribe_attempts == 0 {
                return false;
            }
        }
        true
    }

    fn acknowledge_subscription(&self) {
        let (state, changed) = &*self.state;
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.subscription = SubscriptionState::Connected;
        changed.notify_all();
    }

    fn disconnect_subscription(&self) {
        let (state, changed) = &*self.state;
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.subscription = SubscriptionState::AckBlocked;
        changed.notify_all();
    }

    fn bindings(&self) -> Vec<Binding> {
        self.state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bindings
            .clone()
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

impl Drop for ControlledEnforcer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.state.1.notify_all();
        if let Some(worker) = self.worker.take() {
            worker.join().expect("controlled enforcer should stop");
        }
        remove_fixture_files(&self.socket_path, &self.database_path);
    }
}

fn handle_controlled_connection(
    mut stream: UnixStream,
    state: Arc<(Mutex<ControlledState>, Condvar)>,
    stop: Arc<AtomicBool>,
) {
    let request: Request = read_frame(&mut BufReader::new(&stream))
        .expect("controlled request should decode")
        .expect("controlled request should not be EOF");
    let result = match request.command {
        Command::Health => Ok(ResponseBody::Health(HealthStatus {
            ready: true,
            backend: "controlled".into(),
            message: None,
        })),
        Command::ApplyPolicy(request) => {
            let binding = Binding {
                request,
                state: BindingState::Enforced,
                message: None,
                domain_id: Some(1),
            };
            state
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .bindings
                .push(binding.clone());
            Ok(ResponseBody::Applied(binding))
        }
        Command::ListBindings => Ok(ResponseBody::Bindings(
            state
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .bindings
                .clone(),
        )),
        Command::DetachAgent { .. } => Ok(ResponseBody::Detached),
        Command::SubscribeViolations => {
            let (shared, changed) = &*state;
            let mut shared = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            shared.subscribe_attempts += 1;
            changed.notify_all();
            while shared.subscription == SubscriptionState::AckBlocked
                && !stop.load(Ordering::Acquire)
            {
                shared = changed
                    .wait(shared)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if stop.load(Ordering::Acquire) {
                return;
            }
            drop(shared);
            if write_frame(
                &mut stream,
                &Response {
                    protocol_version: agentsight_enforcement_protocol::PROTOCOL_VERSION,
                    request_id: request.request_id,
                    result: Ok(ResponseBody::Subscribed),
                },
            )
            .is_err()
            {
                return;
            }
            let mut shared = state
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while shared.subscription == SubscriptionState::Connected
                && !stop.load(Ordering::Acquire)
            {
                shared = state
                    .1
                    .wait(shared)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            return;
        }
    };
    write_frame(
        &mut stream,
        &Response {
            protocol_version: agentsight_enforcement_protocol::PROTOCOL_VERSION,
            request_id: request.request_id,
            result,
        },
    )
    .expect("controlled response should encode");
}

fn remove_fixture_files(socket_path: &Path, database_path: &Path) {
    let _ = fs::remove_file(socket_path);
    let _ = fs::remove_file(database_path);
    let _ = fs::remove_file(format!("{}-wal", database_path.display()));
    let _ = fs::remove_file(format!("{}-shm", database_path.display()));
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

fn poll_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    condition()
}

#[test]
fn health_waits_for_subscription_acknowledgement() {
    let fixture = ControlledEnforcer::start();
    let store = EnforcementStore::open(&fixture.database_path)
        .expect("temporary enforcement store should open");
    let coordinator =
        EnforcementCoordinator::new(EnforcementClient::new(&fixture.socket_path), store);
    let ingestion = coordinator
        .start_ingestion()
        .expect("ingestion should start");
    let subscription_attempted = fixture.wait_for_subscribe_attempt();

    let before_ack = coordinator.health().expect("backend health should work");
    fixture.acknowledge_subscription();
    let ready_after_ack = poll_until(|| coordinator.health().is_ok_and(|health| health.ready));
    coordinator.stop_ingestion();
    ingestion.join().expect("ingestion should stop");

    assert!(subscription_attempted);
    assert!(!before_ack.ready);
    assert_eq!(
        before_ack.message.as_deref(),
        Some("violation ingestion is not subscribed")
    );
    assert!(ready_after_ack);
}

#[test]
fn apply_before_subscription_acknowledgement_changes_no_state() {
    let fixture = ControlledEnforcer::start();
    let store = EnforcementStore::open(&fixture.database_path)
        .expect("temporary enforcement store should open");
    let coordinator =
        EnforcementCoordinator::new(EnforcementClient::new(&fixture.socket_path), store);
    let ingestion = coordinator
        .start_ingestion()
        .expect("ingestion should start");
    let subscription_attempted = fixture.wait_for_subscribe_attempt();

    let result = coordinator.apply(fixture.apply_request());
    let backend_bindings = fixture.bindings();
    let persisted_bindings = coordinator
        .bindings()
        .expect("persisted bindings should load");
    coordinator.stop_ingestion();
    fixture.acknowledge_subscription();
    ingestion.join().expect("ingestion should stop");

    assert!(subscription_attempted);
    let error = result.expect_err("apply before ingestion ACK should fail");
    assert!(matches!(
        &error,
        EnforcementCoordinatorError::IngestionUnavailable
    ));
    assert_eq!(error.to_string(), "violation ingestion is not subscribed");
    assert!(backend_bindings.is_empty());
    assert!(persisted_bindings.is_empty());
}

#[test]
fn subscription_disconnect_revokes_readiness_and_rejects_apply() {
    let fixture = ControlledEnforcer::start();
    let store = EnforcementStore::open(&fixture.database_path)
        .expect("temporary enforcement store should open");
    let coordinator =
        EnforcementCoordinator::new(EnforcementClient::new(&fixture.socket_path), store);
    let ingestion = coordinator
        .start_ingestion()
        .expect("ingestion should start");
    let subscription_attempted = fixture.wait_for_subscribe_attempt();
    fixture.acknowledge_subscription();
    let became_ready = poll_until(|| coordinator.health().is_ok_and(|health| health.ready));

    fixture.disconnect_subscription();
    let readiness_revoked = poll_until(|| coordinator.health().is_ok_and(|health| !health.ready));
    let result = coordinator.apply(fixture.apply_request());
    let backend_bindings = fixture.bindings();
    let persisted_bindings = coordinator
        .bindings()
        .expect("persisted bindings should load");
    coordinator.stop_ingestion();
    fixture.acknowledge_subscription();
    ingestion.join().expect("ingestion should stop");

    assert!(subscription_attempted);
    assert!(became_ready);
    assert!(readiness_revoked);
    let error = result.expect_err("apply after disconnect should fail");
    assert!(matches!(
        &error,
        EnforcementCoordinatorError::IngestionUnavailable
    ));
    assert_eq!(error.to_string(), "violation ingestion is not subscribed");
    assert!(backend_bindings.is_empty());
    assert!(persisted_bindings.is_empty());
}

#[test]
fn apply_persists_enforced_state_and_deduplicates_violation() {
    let fixture = TestEnforcer::start();
    let store = EnforcementStore::open(&fixture.database_path)
        .expect("temporary enforcement store should open");
    let coordinator =
        EnforcementCoordinator::new(EnforcementClient::new(&fixture.socket_path), store);
    let ingestion = coordinator
        .start_ingestion()
        .expect("ingestion should start");
    let ready = poll_until(|| coordinator.health().is_ok_and(|health| health.ready));
    let request = fixture.apply_request();
    let binding = coordinator.apply(request.clone());
    assert!(ready);
    assert_eq!(
        binding.expect("mock apply should work").state,
        BindingState::Enforced
    );
    let violation = fixture.violation(&request);
    fixture
        .backend
        .publish_violation(violation.clone())
        .expect("first violation should publish");
    fixture
        .backend
        .publish_violation(violation)
        .expect("duplicate violation should publish");

    let violation_ingested = poll_until(|| {
        coordinator
            .violations(100)
            .expect("violation query should work")
            .len()
            == 1
    });
    assert!(violation_ingested);
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
