#![cfg(feature = "mock-backend")]

use std::fs;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agentsight_enforcement_protocol::{
    ApplyPolicy, BindingState, Command, Effect, Request, Response, ResponseBody, ViolationEvent,
    read_frame, write_frame,
};
use agentsight_enforcer::{BackendError, EnforcementBackend, EnforcerService, MockBackend};
use uuid::Uuid;

fn fixture_apply_policy() -> ApplyPolicy {
    ApplyPolicy {
        binding_id: Uuid::new_v4(),
        agent_id: "test-agent".into(),
        session_id: Some("session-1".into()),
        root_pid: 4242,
        process_start_time: 99,
        policy_id: "test-policy".into(),
        policy_revision: "revision-1".into(),
        policy_dsl: "label AGENT".into(),
    }
}

fn fixture_violation(request: &ApplyPolicy) -> ViolationEvent {
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
        reason: Some("test violation".into()),
        occurred_at_ns: 100,
        observed_at_ns: 101,
        actplane_revision: "mock".into(),
    }
}

#[test]
fn changed_duplicate_conflicts() {
    let backend = MockBackend::new();
    let request = fixture_apply_policy();
    assert!(backend.apply(request.clone()).is_ok());
    assert!(backend.apply(request.clone()).is_ok());
    let mut changed = request;
    changed.policy_revision = "revision-2".into();
    assert!(matches!(
        backend.apply(changed),
        Err(BackendError::BindingConflict(_))
    ));
}

#[test]
fn bind_recovers_a_stale_socket_after_unclean_shutdown() {
    let socket_path =
        PathBuf::from("/tmp").join(format!("agentsight-stale-{}.sock", Uuid::new_v4()));
    let listener = UnixListener::bind(&socket_path).expect("fixture socket should bind");
    drop(listener);

    let service = EnforcerService::bind(&socket_path, Arc::new(MockBackend::new()), None)
        .expect("stale service socket should be replaced");

    drop(service);
    assert!(!socket_path.exists());
}

#[test]
fn bind_never_replaces_a_non_socket_path() {
    let socket_path =
        PathBuf::from("/tmp").join(format!("agentsight-file-{}.sock", Uuid::new_v4()));
    fs::write(&socket_path, b"owned by another service").expect("fixture file should exist");

    let result = EnforcerService::bind(&socket_path, Arc::new(MockBackend::new()), None);

    assert!(result.is_err());
    assert_eq!(
        fs::read(&socket_path).expect("fixture file must remain"),
        b"owned by another service"
    );
    fs::remove_file(socket_path).expect("fixture file should clean up");
}

struct ServiceFixture {
    socket_path: PathBuf,
    backend: Arc<MockBackend>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ServiceFixture {
    fn start() -> Self {
        let socket_path =
            PathBuf::from("/tmp").join(format!("agentsight-enforcer-{}.sock", Uuid::new_v4()));
        let backend = Arc::new(MockBackend::new());
        let service = EnforcerService::bind(&socket_path, Arc::clone(&backend), None)
            .expect("fixture service should bind");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            service
                .serve_until(&worker_stop)
                .expect("fixture service should run");
        });
        wait_for_socket(&socket_path);
        Self {
            socket_path,
            backend,
            stop,
            worker: Some(worker),
        }
    }

    fn call(&self, command: Command) -> Response {
        let request = Request::new(command);
        let mut stream = UnixStream::connect(&self.socket_path)
            .expect("fixture client should connect to service");
        write_frame(&mut stream, &request).expect("fixture request should encode");
        read_frame(&mut BufReader::new(stream))
            .expect("fixture response should decode")
            .expect("fixture response should exist")
    }
}

impl Drop for ServiceFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("fixture service should stop");
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn wait_for_socket(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("fixture service socket did not appear");
}

#[test]
fn uds_service_dispatches_lifecycle_and_streams_violations() {
    let fixture = ServiceFixture::start();

    let health = fixture.call(Command::Health);
    assert!(matches!(health.result, Ok(ResponseBody::Health(_))));

    let apply = fixture_apply_policy();
    let applied = fixture.call(Command::ApplyPolicy(apply.clone()));
    let Ok(ResponseBody::Applied(binding)) = applied.result else {
        panic!("apply must return a binding");
    };
    assert_eq!(binding.state, BindingState::Enforced);

    let listed = fixture.call(Command::ListBindings);
    let Ok(ResponseBody::Bindings(bindings)) = listed.result else {
        panic!("list must return bindings");
    };
    assert_eq!(bindings.len(), 1);

    let request = Request::new(Command::SubscribeViolations);
    let mut stream =
        UnixStream::connect(&fixture.socket_path).expect("fixture subscriber should connect");
    write_frame(&mut stream, &request).expect("subscribe request should encode");
    let mut reader = BufReader::new(stream);
    let subscribed: Response = read_frame(&mut reader)
        .expect("subscribe response should decode")
        .expect("subscribe response should exist");
    assert_eq!(subscribed.request_id, request.request_id);
    assert!(matches!(subscribed.result, Ok(ResponseBody::Subscribed)));

    let violation = fixture_violation(&apply);
    fixture
        .backend
        .publish_violation(violation.clone())
        .expect("fixture violation should publish");
    let streamed: Response = read_frame(&mut reader)
        .expect("violation response should decode")
        .expect("violation response should exist");
    assert_eq!(streamed.request_id, request.request_id);
    assert_eq!(streamed.result, Ok(ResponseBody::Violation(violation)));

    let detached = fixture.call(Command::DetachAgent {
        binding_id: apply.binding_id,
    });
    assert!(matches!(detached.result, Ok(ResponseBody::Detached)));
    let listed = fixture.call(Command::ListBindings);
    assert_eq!(listed.result, Ok(ResponseBody::Bindings(Vec::new())));
}
