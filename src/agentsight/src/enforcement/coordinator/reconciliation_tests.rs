use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agentsight_enforcement_protocol::ProtocolError;
use rusqlite::{Connection, params};

use super::*;
use crate::enforcement::EnforcementStoreError;
use crate::ingestion_readiness::GenerationReadiness;

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "agentsight-reconciliation-{}.db",
                uuid::Uuid::new_v4()
            )),
        }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(format!("{}-wal", self.path.display()));
        let _ = fs::remove_file(format!("{}-shm", self.path.display()));
    }
}

struct ScriptedClient {
    actual: Vec<Binding>,
    apply_results: Mutex<VecDeque<Result<Binding, EnforcementError>>>,
    applied: Mutex<Vec<uuid::Uuid>>,
}

impl ScriptedClient {
    fn new(apply_results: Vec<Result<Binding, EnforcementError>>) -> Self {
        Self {
            actual: Vec::new(),
            apply_results: Mutex::new(apply_results.into()),
            applied: Mutex::new(Vec::new()),
        }
    }

    fn applied(&self) -> Vec<uuid::Uuid> {
        self.applied
            .lock()
            .expect("applied record should not be poisoned")
            .clone()
    }
}

impl DesiredStateClient for ScriptedClient {
    fn bindings(&self) -> Result<Vec<Binding>, EnforcementError> {
        Ok(self.actual.clone())
    }

    fn apply(&self, request: ApplyPolicy) -> Result<Binding, EnforcementError> {
        self.applied
            .lock()
            .expect("applied record should not be poisoned")
            .push(request.binding_id);
        self.apply_results
            .lock()
            .expect("apply results should not be poisoned")
            .pop_front()
            .expect("scripted apply result should exist")
    }

    fn detach(&self, _binding_id: uuid::Uuid) -> Result<(), EnforcementError> {
        Ok(())
    }
}

fn request(id: &str) -> ApplyPolicy {
    ApplyPolicy {
        binding_id: uuid::Uuid::parse_str(id).expect("fixture UUID should parse"),
        agent_id: "reconciliation-agent".into(),
        session_id: Some("reconciliation-session".into()),
        root_pid: 42,
        process_start_time: 99,
        policy_id: "reconciliation-policy".into(),
        policy_revision: "revision-1".into(),
        policy_dsl: "label AGENT".into(),
    }
}

fn desired(request: ApplyPolicy) -> Binding {
    Binding {
        request,
        state: BindingState::Degraded,
        message: Some("backend restarted".into()),
        domain_id: None,
    }
}

fn enforced(request: ApplyPolicy, domain_id: u32) -> Binding {
    Binding {
        request,
        state: BindingState::Enforced,
        message: None,
        domain_id: Some(domain_id),
    }
}

#[test]
fn remote_rejection_fails_only_one_binding_and_allows_readiness() {
    let database = TestDatabase::new();
    let store = EnforcementStore::open(&database.path).expect("test store should open");
    let rejected_request = request("00000000-0000-0000-0000-000000000001");
    let accepted_request = request("00000000-0000-0000-0000-000000000002");
    store
        .upsert_binding(&desired(rejected_request.clone()))
        .expect("rejected desired binding should seed");
    store
        .upsert_binding(&desired(accepted_request.clone()))
        .expect("accepted desired binding should seed");
    let client = ScriptedClient::new(vec![
        Err(EnforcementError::Remote {
            code: "kernel_failure".into(),
            message: "/root/secret.txt\npolicy label CREDENTIAL\u{7}".into(),
        }),
        Ok(enforced(accepted_request.clone(), 7)),
    ]);
    let readiness = GenerationReadiness::new("reconciliation is unavailable");
    let worker = readiness.candidate();
    readiness.install(Arc::clone(&worker));

    reconcile_desired_state(&client, &store).expect("one rejection should not abort the loop");
    assert!(readiness.mark_ready(&worker));

    assert_eq!(
        client.applied(),
        vec![rejected_request.binding_id, accepted_request.binding_id]
    );
    let rejected = store
        .binding(rejected_request.binding_id)
        .expect("rejected binding should load")
        .expect("rejected binding should remain persisted");
    assert_eq!(rejected.state, BindingState::Failed);
    assert_eq!(
        rejected.message.as_deref(),
        Some("enforcer rejected desired binding: kernel attachment failed")
    );
    assert_eq!(rejected.domain_id, None);
    assert_eq!(
        store
            .binding(accepted_request.binding_id)
            .expect("accepted binding should load"),
        Some(enforced(accepted_request, 7))
    );

    let connection = Connection::open(&database.path).expect("raw database should open");
    let raw: (String, String, Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT desired_json, state, message, domain_id
             FROM enforcement_bindings WHERE binding_id = ?1",
            params![rejected_request.binding_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("raw rejected binding should load");
    assert_eq!(serde_json::from_str::<Binding>(&raw.0).unwrap(), rejected);
    assert_eq!(raw.1, "failed");
    assert_eq!(raw.2, rejected.message);
    assert_eq!(raw.3, None);
    assert!(readiness.is_ready());
}

#[test]
fn remote_operator_message_is_bounded_utf8_safe_and_redacted() {
    let oversized = format!("/root/secret.txt {}\npolicy label SECRET", "界".repeat(600));
    let message = remote_rejection_message("kernel_failure", &oversized);

    assert!(message.chars().count() <= 512);
    assert!(message.is_char_boundary(message.len()));
    assert!(!message.chars().any(char::is_control));
    assert!(!message.contains("/root/secret.txt"));
    assert!(!message.contains("policy label SECRET"));
    assert!(!message.contains('界'));
}

#[test]
fn operator_message_sanitizer_replaces_controls_and_has_empty_fallback() {
    assert_eq!(
        sanitize_operator_message("line one\nline two\u{7}"),
        "line one line two"
    );
    assert_eq!(
        sanitize_operator_message("\n\u{7}\t"),
        "enforcer rejected desired binding without operator-safe detail"
    );
    assert_eq!(
        sanitize_operator_message(&"界".repeat(600)).chars().count(),
        512
    );
}

fn reconcile_apply_error(error: EnforcementError) -> EnforcementCoordinatorError {
    let store = EnforcementStore::open(":memory:").expect("test store should open");
    let request = request("00000000-0000-0000-0000-000000000010");
    store
        .upsert_binding(&desired(request))
        .expect("desired binding should seed");
    reconcile_desired_state(&ScriptedClient::new(vec![Err(error)]), &store)
        .expect_err("non-remote errors should abort the generation")
}

#[test]
fn io_protocol_and_response_mismatch_remain_generation_fatal() {
    assert!(matches!(
        reconcile_apply_error(EnforcementError::Io(std::io::Error::other(
            "fixture transport failure"
        ))),
        EnforcementCoordinatorError::Client(EnforcementError::Io(_))
    ));
    assert!(matches!(
        reconcile_apply_error(EnforcementError::Protocol(ProtocolError::MissingNewline)),
        EnforcementCoordinatorError::Client(EnforcementError::Protocol(_))
    ));
    assert!(matches!(
        reconcile_apply_error(EnforcementError::ResponseMismatch {
            expected: uuid::Uuid::new_v4(),
            actual: uuid::Uuid::new_v4(),
        }),
        EnforcementCoordinatorError::Client(EnforcementError::ResponseMismatch { .. })
    ));
}

#[test]
fn acknowledgement_store_failure_remains_generation_fatal() {
    let store = EnforcementStore::open(":memory:").expect("test store should open");
    let request = request("00000000-0000-0000-0000-000000000020");
    store
        .upsert_binding(&desired(request.clone()))
        .expect("desired binding should seed");
    let mut conflicting = request;
    conflicting.policy_revision = "conflicting-revision".into();

    let result = reconcile_desired_state(
        &ScriptedClient::new(vec![Ok(enforced(conflicting, 9))]),
        &store,
    );

    assert!(matches!(
        result,
        Err(EnforcementCoordinatorError::Store(
            EnforcementStoreError::BindingConflict(_)
        ))
    ));
}
