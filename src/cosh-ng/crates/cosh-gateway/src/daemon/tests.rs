use std::io::Cursor;
use std::os::unix::fs::DirBuilderExt;
use std::sync::Arc;

use cosh_gateway_contracts::common::{BoundedName, BoundedOpaque, BoundedText, IdempotencyKey};
use tempfile::TempDir;

use super::*;

fn submit(key: &str) -> SubmitTask {
    SubmitTask {
        request_id: RequestId::new(),
        idempotency_key: IdempotencyKey::new(key).unwrap(),
        intent: BoundedText::new("inspect the failed service").unwrap(),
        target: TargetRef {
            kind: BoundedName::new("local").unwrap(),
            authority: BoundedName::new("test").unwrap(),
            identifier: BoundedOpaque::new("host").unwrap(),
        },
        runtime: RuntimeSelector {
            runtime: BoundedName::new("acp").unwrap(),
            profile: Some(BoundedName::new("codex").unwrap()),
        },
    }
}

fn private_directory(root: &TempDir, name: &str) -> PathBuf {
    let path = root.path().join(name);
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(&path).unwrap();
    path
}

#[test]
fn frame_is_big_endian_bounded_and_round_trips() {
    let request = GatewayRequest::Ping {
        api_version: GATEWAY_API_VERSION.to_owned(),
        request_id: RequestId::new(),
    };
    let mut wire = Vec::new();
    write_frame(&mut wire, &request).unwrap();
    let declared = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
    assert_eq!(declared, wire.len() - 4);
    let decoded = read_frame::<GatewayRequest>(&mut Cursor::new(wire)).unwrap();
    assert_eq!(decoded.request_id(), request.request_id());

    let mut oversized = (u32::try_from(MAX_GATEWAY_FRAME_BYTES).unwrap() + 1)
        .to_be_bytes()
        .to_vec();
    oversized.extend_from_slice(b"{}");
    assert!(matches!(
        read_frame::<GatewayRequest>(&mut Cursor::new(oversized)),
        Err(GatewayDaemonError::Protocol(_))
    ));
}

#[test]
fn request_rejects_unknown_fields() {
    let request_id = RequestId::new();
    let payload = format!(
        r#"{{"command":"ping","api_version":"cosh.gateway.v1","request_id":"{request_id}","actor":"forged"}}"#
    );
    assert!(serde_json::from_str::<GatewayRequest>(&payload).is_err());

    for request in [
        GatewayRequest::Submit {
            api_version: GATEWAY_API_VERSION.to_owned(),
            request: submit("strict-submit"),
        },
        GatewayRequest::Cancel {
            api_version: GATEWAY_API_VERSION.to_owned(),
            request: CancelTask {
                request_id: RequestId::new(),
                idempotency_key: IdempotencyKey::new("strict-cancel").unwrap(),
                task_id: TaskId::new(),
                run_id: RunId::new(),
                expected_revision: None,
            },
        },
    ] {
        let mut value = serde_json::to_value(request).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("actor".to_owned(), serde_json::json!("forged"));
        assert!(serde_json::from_value::<GatewayRequest>(value).is_err());
    }
}

#[test]
fn stable_actor_identity_depends_on_peer_uid() {
    let installation = InstallationId::new();
    let first = actor_id_for_uid(&installation, 1000).unwrap();
    assert_eq!(first, actor_id_for_uid(&installation, 1000).unwrap());
    assert_ne!(first, actor_id_for_uid(&installation, 1001).unwrap());
    assert_ne!(
        first,
        actor_id_for_uid(&InstallationId::new(), 1000).unwrap()
    );
}

#[test]
fn coordinator_replays_submit_and_hides_foreign_tasks() {
    let root = TempDir::new().unwrap();
    let mut coordinator =
        TaskCoordinator::open(root.path().join("gateway.db"), Some(InstallationId::new())).unwrap();
    let owner = actor_id_for_uid(&coordinator.installation_id, 1000).unwrap();
    let request = submit("retry-key");
    let first = coordinator.submit(&owner, request.clone()).unwrap();
    let replay = coordinator.submit(&owner, request).unwrap();
    assert_eq!(first, replay);
    assert!(matches!(
        coordinator.get(
            &actor_id_for_uid(&coordinator.installation_id, 1001).unwrap(),
            &first.task_id
        ),
        Err(GatewayDaemonError::Store(StoreError::TaskNotFound))
    ));
}

#[test]
fn local_client_controls_durable_task_through_authenticated_socket() {
    let root = TempDir::new().unwrap();
    let socket_dir = private_directory(&root, "runtime");
    let socket_path = socket_dir.join("gateway.sock");
    let database_path = root.path().join("gateway.db");
    let mut daemon = GatewayDaemon::bind(GatewayDaemonConfig {
        socket_path: socket_path.clone(),
        database_path,
        installation_id: None,
    })
    .unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = Arc::clone(&shutdown);
    let server = std::thread::spawn(move || daemon.serve_until(&server_shutdown));

    let client = LocalGatewayClient::new(socket_path.clone());
    assert_eq!(client.ping(RequestId::new()).unwrap(), GatewayResult::Pong);
    let request = submit("submit-once");
    let first = client.submit(request.clone()).unwrap();
    let GatewayResult::Task(task) = first else {
        panic!("submit must return a Task")
    };
    assert_eq!(task.state, TaskState::Queued);
    let GatewayResult::Task(replay) = client.submit(request).unwrap() else {
        panic!("replay must return a Task")
    };
    assert_eq!(replay.task_id, task.task_id);

    let GatewayResult::Events(page) = client
        .events(RequestId::new(), task.task_id.clone(), None, 1)
        .unwrap()
    else {
        panic!("events must return a page")
    };
    assert_eq!(page.events.len(), 1);
    assert!(page.has_more);

    let run_id = task.active_run_id.clone().unwrap();
    let GatewayResult::Cancelled(cancelled) = client
        .cancel(CancelTask {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new("cancel-once").unwrap(),
            task_id: task.task_id.clone(),
            run_id: run_id.clone(),
            expected_revision: Some(task.revision),
        })
        .unwrap()
    else {
        panic!("cancel must return a projection")
    };
    assert_eq!(cancelled.state, TaskState::Cancelled);
    let GatewayResult::Cancelled(replayed) = client
        .cancel(CancelTask {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new("cancel-once").unwrap(),
            task_id: task.task_id,
            run_id,
            expected_revision: Some(task.revision),
        })
        .unwrap()
    else {
        panic!("cancel replay must return a projection")
    };
    assert_eq!(replayed, cancelled);

    shutdown.store(true, Ordering::Relaxed);
    server.join().unwrap().unwrap();
    assert!(!socket_path.exists());
}

#[test]
fn database_rejects_installation_identity_substitution() {
    let root = TempDir::new().unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    assert_eq!(coordinator.installation_id, installation);
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator
        .submit(&actor, submit("migration-history"))
        .unwrap();
    drop(coordinator);

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute("DELETE FROM gateway_identity", [])
        .unwrap();
    drop(connection);
    let reopened = TaskCoordinator::open(&database_path, None).unwrap();
    assert_eq!(reopened.installation_id, installation);
    drop(reopened);

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute("DELETE FROM gateway_identity", [])
        .unwrap();
    drop(connection);
    assert!(matches!(
        TaskCoordinator::open(&database_path, Some(InstallationId::new())),
        Err(GatewayDaemonError::Store(StoreError::LedgerConflict { .. }))
    ));
}

#[test]
fn database_rejects_mixed_installation_history() {
    let root = TempDir::new().unwrap();
    let database_path = root.path().join("gateway.db");
    let installation = InstallationId::new();
    let mut coordinator =
        TaskCoordinator::open(&database_path, Some(installation.clone())).unwrap();
    let actor = actor_id_for_uid(&installation, 1000).unwrap();
    coordinator.submit(&actor, submit("mixed-history")).unwrap();
    drop(coordinator);

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM task_events WHERE revision = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut payload = serde_json::from_str::<serde_json::Value>(&payload).unwrap();
    payload["header"]["correlation"]["installation_id"] =
        serde_json::Value::String(InstallationId::new().to_string());
    connection
        .execute(
            "UPDATE task_events SET payload_json = ?1 WHERE revision = 2",
            [serde_json::to_string(&payload).unwrap()],
        )
        .unwrap();
    connection
        .execute("DELETE FROM gateway_identity", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        TaskCoordinator::open(&database_path, None),
        Err(GatewayDaemonError::Store(StoreError::Corrupt { .. }))
    ));
}

#[test]
fn bind_never_unlinks_a_regular_file() {
    let root = TempDir::new().unwrap();
    let socket_dir = private_directory(&root, "runtime");
    let socket_path = socket_dir.join("gateway.sock");
    fs::write(&socket_path, b"do not remove").unwrap();
    let result = GatewayDaemon::bind(GatewayDaemonConfig {
        socket_path: socket_path.clone(),
        database_path: root.path().join("gateway.db"),
        installation_id: None,
    });
    assert!(matches!(result, Err(GatewayDaemonError::UnsafePath { .. })));
    assert_eq!(fs::read(socket_path).unwrap(), b"do not remove");
}

#[test]
fn bind_replaces_only_an_owned_stale_socket() {
    let root = TempDir::new().unwrap();
    let socket_dir = private_directory(&root, "runtime");
    let socket_path = socket_dir.join("gateway.sock");
    let stale = UnixListener::bind(&socket_path).unwrap();
    drop(stale);
    let daemon = GatewayDaemon::bind(GatewayDaemonConfig {
        socket_path: socket_path.clone(),
        database_path: root.path().join("gateway.db"),
        installation_id: None,
    })
    .unwrap();
    assert!(socket_path.exists());
    drop(daemon);
    assert!(!socket_path.exists());
}
