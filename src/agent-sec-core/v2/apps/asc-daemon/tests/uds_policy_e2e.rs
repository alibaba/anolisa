use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use asc_daemon::testing::serve_n;
use asc_daemon::{AppState, TokenVerifier, bind_socket, prepare_auth};
use asc_daemon_core::PolicyService;
use asc_persistence_sqlite::SqlitePolicyStore;
use asc_policy_runtime::testing::FakePolicyAdapter;
use serde_json::{Value, json};

fn call(path: &Path, request: &Value) -> Value {
    let mut stream = UnixStream::connect(path).unwrap();
    let mut bytes = serde_json::to_vec(request).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    serde_json::from_str(response.trim()).unwrap()
}

fn authenticated(method: &str, token: &str, params: &Value) -> Value {
    json!({
        "method": method,
        "params": params,
        "auth": {"scheme": "bearer", "token": token}
    })
}

#[test]
fn uds_slice_stores_authoring_state_and_dispatches_only_to_fake_adapter() {
    let directory = std::env::temp_dir().join(format!("asc-daemon-e2e-{}", uuid::Uuid::new_v4()));
    let token_path = directory.join("policy-admin.token");
    let socket_path = directory.join("daemon.sock");
    prepare_auth(&token_path).unwrap();
    let token = std::fs::read_to_string(&token_path).unwrap();
    let auth = Arc::new(TokenVerifier::load(&token_path).unwrap());
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let adapter = Arc::new(FakePolicyAdapter::default());
    let policy = Arc::new(PolicyService::new(Arc::clone(&store), Arc::clone(&adapter)));
    let state = AppState::new(policy, auth);
    let listener = bind_socket(&socket_path).unwrap();
    let server = thread::spawn(move || serve_n(&listener, &state, 13).unwrap());

    // A peer that disappears before reading must consume only its own connection, not the daemon.
    let disconnected = UnixStream::connect(&socket_path).unwrap();
    disconnected.shutdown(Shutdown::Both).unwrap();
    drop(disconnected);

    let health = call(&socket_path, &json!({"method": "daemon.health"}));
    assert_eq!(health["ok"], true);
    assert_eq!(health["data"]["status"], "ready");
    assert!(health["data"].get("policyAdapter").is_none());

    let unauthenticated = call(
        &socket_path,
        &json!({"method": "policy.templates.list", "params": {}}),
    );
    assert_eq!(unauthenticated["ok"], false);
    assert_eq!(unauthenticated["error"]["code"], "unauthenticated");

    let policy = call(
        &socket_path,
        &authenticated(
            "policy.templates.put",
            &token,
            &json!({
                "policyName": "protect-production-secrets",
                "template": {
                    "kind": "high_sensitivity_read_deny",
                    "files": ["/secrets/**"]
                }
            }),
        ),
    );
    assert_eq!(policy["data"]["disposition"], "STORED");
    let policy_id = policy["data"]["policy"]["policyId"]
        .as_str()
        .unwrap()
        .to_owned();
    uuid::Uuid::parse_str(&policy_id).unwrap();

    let invalid_policy_get = call(
        &socket_path,
        &authenticated(
            "policy.templates.get",
            &token,
            &json!({"id": "prevent-file-deletion", "revision": 1}),
        ),
    );
    assert_eq!(invalid_policy_get["ok"], false);
    assert_eq!(invalid_policy_get["error"]["code"], "bad_request");

    let scope = call(
        &socket_path,
        &authenticated(
            "policy.scopes.put",
            &token,
            &json!({
                "selector": {"kind": "pid", "pid": 4242}
            }),
        ),
    );
    assert_eq!(scope["data"]["disposition"], "STORED");
    let scope_id = scope["data"]["scope"]["scopeId"]
        .as_str()
        .unwrap()
        .to_owned();
    uuid::Uuid::parse_str(&scope_id).unwrap();

    let invalid_policy_ref = call(
        &socket_path,
        &authenticated(
            "policy.bindings.put",
            &token,
            &json!({
                "policyRef": {"id": "prevent-file-deletion", "revision": 1},
                "scopeRef": {"id": scope_id.clone(), "revision": 1}
            }),
        ),
    );
    assert_eq!(invalid_policy_ref["ok"], false);
    assert_eq!(invalid_policy_ref["error"]["code"], "bad_request");

    let binding = call(
        &socket_path,
        &authenticated(
            "policy.bindings.put",
            &token,
            &json!({
                "policyRef": {"id": policy_id.clone(), "revision": 1},
                "scopeRef": {"id": scope_id.clone(), "revision": 1}
            }),
        ),
    );
    assert_eq!(binding["data"]["disposition"], "ACCEPTED");
    assert_eq!(binding["data"]["binding"]["desiredState"], "READY");
    assert!(binding["data"].get("operation").is_none());
    assert!(!binding.to_string().contains("operationId"));
    assert!(!binding.to_string().contains(&token));
    let binding_id = binding["data"]["binding"]["bindingId"]
        .as_str()
        .unwrap()
        .to_owned();
    uuid::Uuid::parse_str(&binding_id).unwrap();

    for _ in 0..100 {
        if !adapter.commands().is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    let policies = call(
        &socket_path,
        &authenticated(
            "policy.templates.list",
            &token,
            &json!({"limit": 100, "offset": 0}),
        ),
    );
    assert_eq!(policies["data"]["total"], 1);
    assert_eq!(policies["data"]["items"][0]["policyId"], policy_id);

    let scopes = call(
        &socket_path,
        &authenticated(
            "policy.scopes.list",
            &token,
            &json!({"limit": 100, "offset": 0}),
        ),
    );
    assert_eq!(scopes["data"]["total"], 1);
    assert_eq!(scopes["data"]["items"][0]["scopeId"], scope_id);

    let bindings = call(
        &socket_path,
        &authenticated(
            "policy.bindings.list",
            &token,
            &json!({"limit": 100, "offset": 0}),
        ),
    );
    assert_eq!(bindings["data"]["total"], 1);
    assert_eq!(bindings["data"]["items"][0]["bindingId"], binding_id);

    let binding_record = call(
        &socket_path,
        &authenticated(
            "policy.bindings.get",
            &token,
            &json!({"id": binding_id.clone()}),
        ),
    );
    assert!(binding_record["data"].get("executionDomainId").is_none());
    assert_eq!(binding_record["data"]["scope"]["selector"]["pid"], 4242);
    assert!(binding_record["data"].get("pid").is_none());
    assert!(binding_record["data"].get("cgroupId").is_none());

    let removed_operation_api = call(
        &socket_path,
        &json!({
            "method": "policy.operations.get",
            "params": {"id": "33333333-3333-4333-8333-333333333333"}
        }),
    );
    assert_eq!(removed_operation_api["ok"], false);
    assert_eq!(removed_operation_api["error"]["code"], "unknown_method");

    server.join().unwrap();
    let commands = adapter.commands();
    assert_eq!(commands.len(), 1);
    uuid::Uuid::parse_str(commands[0].operation_id.as_str()).unwrap();
    assert_eq!(commands[0].binding.policy.policy_id.as_str(), policy_id);
    assert_eq!(commands[0].binding.scope.scope_id.as_str(), scope_id);

    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&token_path);
    let _ = std::fs::remove_dir(&directory);
}
