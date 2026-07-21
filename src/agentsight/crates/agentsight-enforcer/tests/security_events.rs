#![cfg(feature = "mock-backend")]

use std::fs;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agentsight_enforcement_protocol::{
    ApplyPolicy, Command, PolicyMode, Request, Response, ResponseBody, SecurityEvent,
    SecurityEventKind, read_frame, write_frame,
};
use agentsight_enforcer::{EnforcementBackend, EnforcerService, MockBackend};
use uuid::Uuid;

struct EnforcerFixture {
    socket_path: PathBuf,
    backend: Arc<MockBackend>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl EnforcerFixture {
    fn start() -> Self {
        let socket_path =
            PathBuf::from("/tmp").join(format!("agentsight-security-{}.sock", Uuid::new_v4()));
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

    fn audit_policy(&self) -> ApplyPolicy {
        ApplyPolicy {
            binding_id: Uuid::new_v4(),
            agent_id: "hermes-test".into(),
            session_id: Some("session-1".into()),
            root_pid: 4242,
            process_start_time: 99,
            policy_id: "credential-exfiltration".into(),
            policy_revision: "3".into(),
            policy_dsl: "mode audit".into(),
        }
    }

    fn apply_policy(&self, request: ApplyPolicy) {
        self.backend
            .apply(request)
            .expect("fixture policy should apply");
    }

    fn subscribe_security_events(&self) -> (Uuid, BufReader<UnixStream>) {
        let request = Request::new(Command::SubscribeSecurityEvents);
        let mut stream = UnixStream::connect(&self.socket_path)
            .expect("fixture subscriber should connect to service");
        write_frame(&mut stream, &request).expect("subscribe request should encode");
        let mut reader = BufReader::new(stream);
        let subscribed: Response = read_frame(&mut reader)
            .expect("subscribe response should decode")
            .expect("subscribe response should exist");
        assert_eq!(subscribed.request_id, request.request_id);
        assert!(matches!(subscribed.result, Ok(ResponseBody::Subscribed)));
        (request.request_id, reader)
    }
}

impl Drop for EnforcerFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.socket_path).and_then(|mut stream| stream.write_all(b"\n"));
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

fn read_security_events(
    reader: &mut BufReader<UnixStream>,
    request_id: Uuid,
    count: usize,
) -> Vec<SecurityEvent> {
    (0..count)
        .map(|_| {
            let response: Response = read_frame(reader)
                .expect("security response should decode")
                .expect("security response should exist");
            assert_eq!(response.request_id, request_id);
            let Ok(ResponseBody::SecurityEvent(event)) = response.result else {
                panic!("subscription frame must contain a security event");
            };
            event
        })
        .collect()
}

#[test]
fn subscription_returns_ordered_source_taint_sink_and_decision() {
    let fixture = EnforcerFixture::start();
    let policy = fixture.audit_policy();
    let binding_id = policy.binding_id;
    fixture.apply_policy(policy);
    let (request_id, mut subscription) = fixture.subscribe_security_events();

    fixture
        .backend
        .emit_credential_exfiltration(
            binding_id,
            "/home/test/.ssh/id_rsa",
            "198.51.100.10:443",
        )
        .expect("fixture event chain should publish");

    let events = read_security_events(&mut subscription, request_id, 4);
    let SecurityEventKind::FileAction(source) = &events[0].kind else {
        panic!("first fixture event must be the source file action");
    };
    assert_eq!(source.path, "~/.ssh/id_rsa");
    assert!(matches!(
        events[1].kind,
        SecurityEventKind::TaintTransition(_)
    ));
    assert!(matches!(
        events[2].kind,
        SecurityEventKind::NetworkAction(_)
    ));
    let SecurityEventKind::PolicyDecision(decision) = &events[3].kind else {
        panic!("fourth fixture event must be the decision");
    };
    assert_eq!(decision.mode, PolicyMode::Audit);
    assert!(!decision.blocked);
    assert_eq!(decision.source_event_id, events[0].event_id);
    assert_eq!(decision.sink_event_id, events[2].event_id);
    assert_eq!(events[0].occurred_at_ns + 1, events[1].occurred_at_ns);
    assert_eq!(events[1].occurred_at_ns + 1, events[2].occurred_at_ns);
    assert_eq!(events[2].occurred_at_ns + 1, events[3].occurred_at_ns);
}

#[test]
fn enforce_mode_preserves_the_observed_eperm_result() {
    let fixture = EnforcerFixture::start();
    let mut policy = fixture.audit_policy();
    policy.policy_dsl = "mode enforce".into();
    let binding_id = policy.binding_id;
    fixture.apply_policy(policy);
    let (request_id, mut subscription) = fixture.subscribe_security_events();

    fixture
        .backend
        .emit_credential_exfiltration(
            binding_id,
            "/home/test/.ssh/id_rsa",
            "198.51.100.10:443",
        )
        .expect("fixture event chain should publish");

    let events = read_security_events(&mut subscription, request_id, 4);
    let SecurityEventKind::PolicyDecision(decision) = &events[3].kind else {
        panic!("fourth fixture event must be the decision");
    };
    assert_eq!(decision.mode, PolicyMode::Enforce);
    assert!(decision.blocked);
    assert_eq!(decision.errno, Some(libc::EPERM));
}
