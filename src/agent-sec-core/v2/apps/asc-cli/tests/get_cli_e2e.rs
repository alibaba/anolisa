use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;

use asc_cli::{Cli, CliError, execute};
use asc_daemon::testing::serve_n;
use asc_daemon::{AppState, TokenVerifier, bind_socket, prepare_auth};
use asc_daemon_client::ClientError;
use asc_daemon_core::PolicyService;
use asc_daemon_protocol::PutPolicyParams;
use asc_foundation_types::{ResourceId, Revision};
use asc_pap::ScopeSelector;
use asc_persistence_sqlite::SqlitePolicyStore;
use asc_policy_engine::PolicyTemplate;
use asc_policy_runtime::testing::FakePolicyAdapter;
use clap::Parser as _;
use serde_json::{Value, json};
use uuid::Uuid;

const POLICY_TEMPLATE_EXAMPLES: [&str; 3] = [
    "template-high-sensitive-read.json",
    "template-prevent-file-deletion.json",
    "template-low-sensitivity-egress.json",
];

#[test]
fn every_policy_template_example_completes_a_real_cli_put_then_get() {
    let directory = unique_directory("policy-template-examples");
    let token_file = directory.join("policy-admin.token");
    let socket = directory.join("daemon.sock");
    prepare_auth(&token_file).unwrap();
    let auth = Arc::new(TokenVerifier::load(&token_file).unwrap());
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let adapter = Arc::new(FakePolicyAdapter::default());
    let policy = Arc::new(PolicyService::new(store, adapter));
    let state = AppState::new(policy, auth);
    let listener = bind_socket(&socket).unwrap();
    let server = thread::spawn(move || {
        serve_n(&listener, &state, POLICY_TEMPLATE_EXAMPLES.len() * 2).unwrap();
    });

    for fixture_name in POLICY_TEMPLATE_EXAMPLES {
        let fixture = fixture_path(fixture_name);
        let input: PutPolicyParams = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
        assert!(input.policy_id.is_none());
        let policy_name = input.policy_name.clone();
        let template = serde_json::to_value(input.template).unwrap();

        let stored = invoke(
            &socket,
            &token_file,
            &[
                "policy",
                "template",
                "put",
                "--file",
                fixture.to_str().unwrap(),
            ],
        );
        assert_eq!(stored["disposition"], "STORED");
        let policy_id = stored["policy"]["policyId"].as_str().unwrap();
        Uuid::parse_str(policy_id).unwrap();
        assert_eq!(stored["policy"]["policyName"], policy_name);
        assert_eq!(stored["policy"]["template"], template);
        let revision = stored["policy"]["revision"].as_u64().unwrap().to_string();

        let fetched = invoke(
            &socket,
            &token_file,
            &[
                "policy",
                "template",
                "get",
                policy_id,
                "--revision",
                &revision,
            ],
        );
        assert_eq!(fetched, stored["policy"]);
    }

    server.join().unwrap();
    cleanup(&directory);
}

#[test]
fn template_put_converges_complete_state_without_a_caller_revision() {
    let directory = unique_directory("put-template");
    let token_file = directory.join("policy-admin.token");
    let input_file = directory.join("policy-template.json");
    let socket = directory.join("daemon.sock");
    prepare_auth(&token_file).unwrap();
    fs::write(
        &input_file,
        serde_json::to_vec(&json!({
            "policyName": "policy-forwarded-by-cli",
            "template": {
                "kind": "high_sensitivity_read_deny",
                "files": ["/secrets/**"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let auth = Arc::new(TokenVerifier::load(&token_file).unwrap());
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let adapter = Arc::new(FakePolicyAdapter::default());
    let policy = Arc::new(PolicyService::new(store, adapter));
    let state = AppState::new(policy, auth);
    let listener = bind_socket(&socket).unwrap();
    let server = thread::spawn(move || serve_n(&listener, &state, 9).unwrap());

    let stored = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "template",
            "put",
            "--file",
            input_file.to_str().unwrap(),
        ],
    );
    assert_eq!(stored["disposition"], "STORED");
    let policy_id = stored["policy"]["policyId"].as_str().unwrap();
    Uuid::parse_str(policy_id).unwrap();
    assert_eq!(stored["policy"]["policyName"], "policy-forwarded-by-cli");
    assert_eq!(stored["policy"]["revision"], 1);
    assert_eq!(stored["policy"]["canonicalPolicy"]["policyId"], policy_id);
    assert!(
        stored["policy"]["canonicalPolicy"]
            .get("policyName")
            .is_none()
    );

    fs::write(
        &input_file,
        serde_json::to_vec(&json!({
            "policyId": policy_id,
            "policyName": "policy-forwarded-by-cli",
            "template": {
                "kind": "high_sensitivity_read_deny",
                "files": ["/secrets/**"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let repeated_update = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "template",
            "put",
            "--file",
            input_file.to_str().unwrap(),
        ],
    );
    assert_eq!(repeated_update, stored);

    fs::write(
        &input_file,
        serde_json::to_vec(&json!({
            "policyId": policy_id,
            "policyName": "renamed-policy-forwarded-by-cli",
            "template": {
                "kind": "prevent_file_deletion",
                "files": ["/important/**"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let changed = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "template",
            "put",
            "--file",
            input_file.to_str().unwrap(),
        ],
    );
    assert_eq!(
        changed["policy"]["policyName"],
        "renamed-policy-forwarded-by-cli"
    );
    assert_eq!(changed["policy"]["revision"], 2);
    assert_eq!(changed["policy"]["canonicalPolicy"]["revision"], 2);

    let original = invoke(
        &socket,
        &token_file,
        &["policy", "template", "get", policy_id, "--revision", "1"],
    );
    assert_eq!(original, stored["policy"]);

    let latest_written = invoke(
        &socket,
        &token_file,
        &["policy", "template", "get", policy_id, "--revision", "2"],
    );
    assert_eq!(latest_written, changed["policy"]);

    let deleted = invoke(
        &socket,
        &token_file,
        &["policy", "template", "delete", policy_id, "--revision", "2"],
    );
    assert_eq!(deleted["disposition"], "DELETED");
    assert_eq!(deleted["policy"], changed["policy"]);
    assert!(deleted["policy"].get("retired").is_none());

    let deleted_get = parse(
        &socket,
        &token_file,
        &["policy", "template", "get", policy_id, "--revision", "2"],
    );
    let error = execute(deleted_get, &mut Vec::new()).unwrap_err();
    assert!(matches!(
        error,
        CliError::Rejected { ref code, .. } if code == "not_found"
    ));

    let retained_original = invoke(
        &socket,
        &token_file,
        &["policy", "template", "get", policy_id, "--revision", "1"],
    );
    assert_eq!(retained_original, stored["policy"]);

    let after_gap = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "template",
            "put",
            "--file",
            input_file.to_str().unwrap(),
        ],
    );
    assert_eq!(after_gap["policy"]["revision"], 3);
    assert_eq!(after_gap["policy"]["canonicalPolicy"]["revision"], 3);

    server.join().unwrap();
    cleanup(&directory);
}

#[test]
fn template_put_rejects_a_non_dto_file_before_connecting_to_the_daemon() {
    let directory = unique_directory("invalid-template");
    let token_file = directory.join("policy-admin.token");
    let input_file = directory.join("policy-template.json");
    prepare_auth(&token_file).unwrap();
    fs::write(
        &input_file,
        br#"{"policyName":"invalid-template","template":{"kind":"high_sensitivity_read_deny","files":["/secrets/**"],"targetDsl":"not-allowed"}}"#,
    )
    .unwrap();
    let cli = parse(
        &directory.join("missing.sock"),
        &token_file,
        &[
            "policy",
            "template",
            "put",
            "--file",
            input_file.to_str().unwrap(),
        ],
    );
    assert!(matches!(
        execute(cli, &mut Vec::new()),
        Err(CliError::InvalidInput)
    ));
    cleanup(&directory);
}

#[test]
fn scope_and_binding_writes_use_the_real_daemon_protocol() {
    let directory = unique_directory("scope-binding-writes");
    let token_file = directory.join("policy-admin.token");
    let socket = directory.join("daemon.sock");
    prepare_auth(&token_file).unwrap();

    let auth = Arc::new(TokenVerifier::load(&token_file).unwrap());
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let adapter = Arc::new(FakePolicyAdapter::default());
    let policy = Arc::new(PolicyService::new(store, adapter));
    let prepared = policy
        .put_policy(
            None,
            "scope-binding-e2e",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/secrets/**".to_owned()],
            },
        )
        .unwrap();

    let state = AppState::new(policy, auth);
    let listener = bind_socket(&socket).unwrap();
    let server = thread::spawn(move || serve_n(&listener, &state, 7).unwrap());

    let stored_scope = invoke(
        &socket,
        &token_file,
        &["policy", "scope", "put", "--pid", "4242"],
    );
    assert_eq!(stored_scope["disposition"], "STORED");
    let scope_id = stored_scope["scope"]["scopeId"].as_str().unwrap();
    Uuid::parse_str(scope_id).unwrap();
    assert_eq!(stored_scope["scope"]["selector"]["kind"], "pid");
    assert_eq!(stored_scope["scope"]["selector"]["pid"], 4242);
    let fetched_scope = invoke(
        &socket,
        &token_file,
        &["policy", "scope", "get", scope_id, "--revision", "1"],
    );
    assert_eq!(fetched_scope, stored_scope["scope"]);

    let accepted_binding = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "binding",
            "put",
            "--policy-id",
            prepared.policy_id.as_str(),
            "--policy-revision",
            "1",
            "--scope-id",
            scope_id,
            "--scope-revision",
            "1",
        ],
    );
    assert_eq!(accepted_binding["disposition"], "ACCEPTED");
    assert_eq!(accepted_binding["binding"]["desiredState"], "READY");
    assert!(accepted_binding.get("operation").is_none());
    let binding_id = accepted_binding["binding"]["bindingId"].as_str().unwrap();
    Uuid::parse_str(binding_id).unwrap();
    let fetched_binding = invoke(
        &socket,
        &token_file,
        &["policy", "binding", "get", binding_id],
    );
    assert_eq!(fetched_binding["bindingRevision"], 1);
    assert_eq!(fetched_binding["desiredState"], "READY");

    let accepted_delete = invoke(
        &socket,
        &token_file,
        &["policy", "binding", "delete", binding_id],
    );
    assert_eq!(accepted_delete["disposition"], "ACCEPTED");
    assert_eq!(accepted_delete["binding"]["desiredState"], "ABSENT");
    assert_eq!(accepted_delete["binding"]["bindingId"], binding_id);
    assert!(accepted_delete.get("operation").is_none());
    let absent_binding = invoke(
        &socket,
        &token_file,
        &["policy", "binding", "get", binding_id],
    );
    assert_eq!(absent_binding["bindingRevision"], 2);
    assert_eq!(absent_binding["desiredState"], "ABSENT");

    let deleted_scope = invoke(
        &socket,
        &token_file,
        &["policy", "scope", "delete", scope_id, "--revision", "1"],
    );
    assert_eq!(deleted_scope["disposition"], "DELETED");
    assert_eq!(deleted_scope["scope"]["scopeId"], scope_id);

    server.join().unwrap();
    cleanup(&directory);
}

#[test]
fn template_scope_and_binding_get_use_the_real_daemon_protocol() {
    let directory = unique_directory("get");
    let token_file = directory.join("policy-admin.token");
    let socket = directory.join("daemon.sock");
    prepare_auth(&token_file).unwrap();
    let auth = Arc::new(TokenVerifier::load(&token_file).unwrap());
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let adapter = Arc::new(FakePolicyAdapter::default());
    let policy = Arc::new(PolicyService::new(Arc::clone(&store), adapter));
    let (policy_id, scope_id, binding_id) = seed(&policy);
    let state = AppState::new(policy, auth);
    let listener = bind_socket(&socket).unwrap();
    let server = thread::spawn(move || serve_n(&listener, &state, 7).unwrap());

    let template = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "template",
            "get",
            policy_id.as_str(),
            "--revision",
            "1",
        ],
    );
    assert_eq!(template["policyId"], policy_id.as_str());
    assert_eq!(template["canonicalPolicy"]["policyId"], policy_id.as_str());

    let scope = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "scope",
            "get",
            scope_id.as_str(),
            "--revision",
            "1",
        ],
    );
    assert_eq!(scope["scopeId"], scope_id.as_str());
    assert_eq!(scope["template"]["kind"], "execution_domain");

    let binding = invoke(
        &socket,
        &token_file,
        &["policy", "binding", "get", binding_id.as_str()],
    );
    assert_eq!(binding["bindingId"], binding_id.as_str());
    assert_eq!(binding["policy"]["policyId"], policy_id.as_str());
    assert_eq!(binding["scope"]["scopeId"], scope_id.as_str());

    let deleted_policy = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "template",
            "delete",
            policy_id.as_str(),
            "--revision",
            "1",
        ],
    );
    assert_eq!(deleted_policy["disposition"], "DELETED");

    let deleted_scope = invoke(
        &socket,
        &token_file,
        &[
            "policy",
            "scope",
            "delete",
            scope_id.as_str(),
            "--revision",
            "1",
        ],
    );
    assert_eq!(deleted_scope["disposition"], "DELETED");

    let retained_binding = invoke(
        &socket,
        &token_file,
        &["policy", "binding", "get", binding_id.as_str()],
    );
    assert_eq!(retained_binding["policy"]["policyId"], policy_id.as_str());
    assert_eq!(retained_binding["scope"]["scopeId"], scope_id.as_str());

    let missing = parse(
        &socket,
        &token_file,
        &[
            "policy",
            "binding",
            "get",
            "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
        ],
    );
    let error = execute(missing, &mut Vec::new()).unwrap_err();
    assert!(matches!(
        error,
        CliError::Rejected { ref code, .. } if code == "not_found"
    ));

    server.join().unwrap();
    cleanup(&directory);
}

#[test]
fn list_queries_use_the_real_daemon_protocol() {
    let directory = unique_directory("list");
    let token_file = directory.join("policy-admin.token");
    let socket = directory.join("daemon.sock");
    prepare_auth(&token_file).unwrap();
    let auth = Arc::new(TokenVerifier::load(&token_file).unwrap());
    let store = Arc::new(SqlitePolicyStore::memory().unwrap());
    let adapter = Arc::new(FakePolicyAdapter::default());
    let policy = Arc::new(PolicyService::new(store, adapter));
    let (policy_id, scope_id, binding_id) = seed(&policy);
    let state = AppState::new(policy, auth);
    let listener = bind_socket(&socket).unwrap();
    let server = thread::spawn(move || serve_n(&listener, &state, 3).unwrap());

    let templates = invoke(
        &socket,
        &token_file,
        &["policy", "template", "list", "--limit", "1"],
    );
    assert_eq!(templates["total"], 1);
    assert_eq!(templates["items"][0]["policyId"], policy_id.as_str());

    let scopes = invoke(
        &socket,
        &token_file,
        &["policy", "scope", "list", "--offset", "0"],
    );
    assert_eq!(scopes["total"], 1);
    assert_eq!(scopes["items"][0]["scopeId"], scope_id.as_str());

    let bindings = invoke(&socket, &token_file, &["policy", "binding", "list"]);
    assert_eq!(bindings["total"], 1);
    assert_eq!(bindings["items"][0]["bindingId"], binding_id.as_str());

    server.join().unwrap();
    cleanup(&directory);
}

#[test]
fn missing_daemon_returns_unavailable_without_a_local_fallback() {
    let directory = unique_directory("unavailable");
    let token_file = directory.join("policy-admin.token");
    prepare_auth(&token_file).unwrap();
    let socket = directory.join("missing.sock");
    let cli = parse(
        &socket,
        &token_file,
        &[
            "policy",
            "binding",
            "get",
            "6efed5ea-47c9-4b14-8e86-888f2ad88fc7",
        ],
    );
    let error = execute(cli, &mut Vec::new()).unwrap_err();
    assert!(matches!(
        error,
        CliError::Client(ClientError::DaemonUnavailable)
    ));
    cleanup(&directory);
}

fn seed(
    policy: &PolicyService<SqlitePolicyStore, FakePolicyAdapter>,
) -> (ResourceId, ResourceId, ResourceId) {
    let prepared = policy
        .put_policy(
            None,
            "seed-policy",
            &PolicyTemplate::HighSensitivityReadDeny {
                files: vec!["/secrets/**".to_owned()],
            },
        )
        .unwrap();
    let scope = policy
        .put_scope(None, &ScopeSelector::Pid { pid: 4242 })
        .unwrap();
    let binding = policy
        .put_binding(
            None,
            &prepared.policy_id,
            revision(1),
            &scope.scope_id,
            revision(1),
        )
        .unwrap();
    (prepared.policy_id, scope.scope_id, binding.binding_id)
}

fn invoke(socket: &Path, token_file: &Path, command: &[&str]) -> Value {
    let cli = parse(socket, token_file, command);
    let mut output = Vec::new();
    execute(cli, &mut output).unwrap();
    serde_json::from_slice(&output).unwrap()
}

fn parse(socket: &Path, token_file: &Path, command: &[&str]) -> Cli {
    let mut arguments = vec![
        "asc-cli".to_owned(),
        "--socket".to_owned(),
        socket.display().to_string(),
        "--token-file".to_owned(),
        token_file.display().to_string(),
    ];
    arguments.extend(command.iter().map(ToString::to_string));
    Cli::try_parse_from(arguments).unwrap()
}

fn revision(value: u64) -> Revision {
    Revision::new(value).unwrap()
}

fn unique_directory(suffix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("asc-cli-{suffix}-{}-{nonce}", std::process::id()))
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn cleanup(directory: &Path) {
    let _ = fs::remove_file(directory.join("daemon.sock"));
    let _ = fs::remove_file(directory.join("policy-admin.token"));
    let _ = fs::remove_file(directory.join("policy-template.json"));
    let _ = fs::remove_dir(directory);
}
