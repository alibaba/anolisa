use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asc_daemon::{BootstrapConfig, serve};
use asc_daemon_core::{PeerCredentials, PrincipalPolicy, PrincipalRole};
use asc_daemon_handler::{DaemonDispatcher, JsonRejectionEncoder};
use asc_daemon_service::{
    DispatchError, DispatchRequest, RequestDispatcher, ResponseDisposition, ShutdownToken,
};
use asc_pap::PapService;
use asc_pap_repository_memory::ProcessLocalPapRepository;
use asc_policy_engine::PolicyTemplateCompiler;
use serde_json::{Value, json};
use uuid::Uuid;

mod common;

struct TestPolicy(PrincipalRole);
impl PrincipalPolicy for TestPolicy {
    fn role_for(&self, _peer: PeerCredentials) -> PrincipalRole {
        self.0
    }
}

struct RecordingDispatcher {
    inner: Arc<dyn RequestDispatcher>,
    requests: Arc<Mutex<Vec<Value>>>,
}
impl RequestDispatcher for RecordingDispatcher {
    fn dispatch(
        &self,
        request: DispatchRequest,
        response: &mut dyn std::io::Write,
    ) -> Result<ResponseDisposition, DispatchError> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&request.payload).unwrap());
        self.inner.dispatch(request, response)
    }
}

async fn start(
    socket: &Path,
    role: PrincipalRole,
) -> (
    ShutdownToken,
    tokio::task::JoinHandle<()>,
    Arc<Mutex<Vec<Value>>>,
) {
    let application = PapService::new(
        Arc::new(ProcessLocalPapRepository::default()),
        Arc::new(PolicyTemplateCompiler),
    );
    let inner = Arc::new(DaemonDispatcher::new(
        application,
        Arc::new(TestPolicy(role)),
    ));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let dispatcher = Arc::new(RecordingDispatcher {
        inner,
        requests: Arc::clone(&requests),
    });
    let config = BootstrapConfig::new(socket);
    let shutdown = ShutdownToken::new();
    let service_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        serve(
            config,
            dispatcher,
            Arc::new(JsonRejectionEncoder),
            service_shutdown,
        )
        .await
        .unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    (shutdown, task, requests)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_cli_processes_execute_the_complete_frozen_pap_crud_scenario() {
    let directory = common::Directory::new();
    let socket = directory.0.join("daemon.sock");
    let (shutdown, task, requests) = start(&socket, PrincipalRole::PolicyAdministrator).await;
    let fixture: Value = serde_json::from_str(common::SCENARIO).unwrap();
    let mut variables = BTreeMap::new();
    let mut expected_requests = Vec::new();
    for step in fixture["steps"].as_array().unwrap() {
        let request = common::expand(&step["request"], &fixture["objects"], &variables);
        let args = common::args_for(&request, &directory.0, &socket);
        let output = tokio::task::spawn_blocking(move || common::run(&args))
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            step["name"],
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        let wrapper = json!({"result":result});
        for capture in step["captures"].as_array().unwrap() {
            let value = wrapper
                .pointer(capture["pointer"].as_str().unwrap())
                .unwrap()
                .clone();
            Uuid::parse_str(value.as_str().unwrap()).unwrap();
            assert!(
                variables
                    .insert(capture["name"].as_str().unwrap().to_owned(), value)
                    .is_none()
            );
        }
        assert_eq!(
            result,
            common::expand(
                &step["expectedResponse"]["result"],
                &fixture["objects"],
                &variables
            ),
            "{}",
            step["name"]
        );
        expected_requests.push(request);
    }
    assert_ne!(variables["policy_id"], variables["scope_id"]);
    assert_ne!(variables["binding_id"], variables["scope_id"]);
    assert_ne!(variables["binding_id"], variables["policy_id"]);
    assert_eq!(*requests.lock().unwrap(), expected_requests);
    let request = json!({"method":"policy.templates.get","params":{"id":variables["policy_id"],"revision":2}});
    let args = common::args_for(&request, &directory.0, &socket);
    let output = tokio::task::spawn_blocking(move || common::run(&args))
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "not_found");
    Uuid::parse_str(error["requestId"].as_str().unwrap()).unwrap();
    shutdown.request();
    task.await.unwrap();
    assert!(!socket.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_cli_cannot_read_or_modify_any_pap_resource() {
    let directory = common::Directory::new();
    let socket = directory.0.join("daemon.sock");
    let (shutdown, task, _) = start(&socket, PrincipalRole::LocalUser).await;
    let methods: Value = serde_json::from_str(common::METHODS).unwrap();
    for row in methods.as_array().unwrap() {
        let args = common::args_for(row, &directory.0, &socket);
        let output = tokio::task::spawn_blocking(move || common::run(&args))
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "permission_denied");
        assert!(!error["error"]["message"].as_str().unwrap().is_empty());
        Uuid::parse_str(error["requestId"].as_str().unwrap()).unwrap();
    }
    shutdown.request();
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domain_validation_and_pagination_are_owned_by_the_daemon() {
    let directory = common::Directory::new();
    let socket = directory.0.join("daemon.sock");
    let (shutdown, task, requests) = start(&socket, PrincipalRole::PolicyAdministrator).await;
    let request = json!({"method":"policy.templates.create","params":{"policyName":"", "template":{"kind":"prevent_file_deletion","files":["/work"]}}});
    let args = common::args_for(&request, &directory.0, &socket);
    let output = tokio::task::spawn_blocking(move || common::run(&args))
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["error"]["code"],
        "invalid_argument"
    );
    for pid in 1..=3 {
        let request =
            json!({"method":"policy.scopes.create","params":{"selector":{"kind":"pid","pid":pid}}});
        let args = common::args_for(&request, &directory.0, &socket);
        assert!(
            tokio::task::spawn_blocking(move || common::run(&args))
                .await
                .unwrap()
                .status
                .success()
        );
    }
    let request = json!({"method":"policy.scopes.list","params":{"limit":1,"offset":1}});
    let args = common::args_for(&request, &directory.0, &socket);
    let output = tokio::task::spawn_blocking(move || common::run(&args))
        .await
        .unwrap();
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["total"], 3);
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        requests.lock().unwrap().len(),
        5,
        "CLI must not auto-page or perform hidden reads"
    );
    shutdown.request();
    task.await.unwrap();
}
