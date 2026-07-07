use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn binary_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("cosh-core");
    path
}

fn run_registry_request(domain: &str, action: &str, params: Value) -> Value {
    let home = tempfile::tempdir().expect("temp home");
    run_registry_request_with_context(domain, action, params, home.path(), None)
}

fn run_registry_request_with_context(
    domain: &str,
    action: &str,
    params: Value,
    home: &Path,
    cwd: Option<&Path>,
) -> Value {
    let bin = binary_path();
    let request = serde_json::json!({
        "type": "registry_request",
        "request_id": "test-1",
        "domain": domain,
        "action": action,
        "params": params,
    });

    let mut command = Command::new(&bin);
    command
        .arg("--registry")
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn {}: {e}", bin.display()));

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|e| panic!("bad JSON: {e}: {l}")))
        .next()
        .expect("expected at least one response line")
}

#[test]
fn registry_extensions_list_returns_success() {
    let resp = run_registry_request("extensions", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["request_id"], "test-1");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_array(), "data should be array: {resp}");
}

#[test]
fn registry_skills_list_returns_success() {
    let resp = run_registry_request("skills", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["request_id"], "test-1");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_array(), "data should be array: {resp}");
}

#[test]
fn registry_hooks_list_returns_success() {
    let resp = run_registry_request("hooks", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["request_id"], "test-1");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_array(), "data should be array: {resp}");
}

#[test]
fn registry_auth_state_merges_user_auth_with_project_preferences() {
    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    let home_config_dir = home.path().join(".copilot-shell");
    let project_config_dir = project.path().join(".copilot-shell");
    std::fs::create_dir_all(&home_config_dir).unwrap();
    std::fs::create_dir_all(&project_config_dir).unwrap();
    std::fs::write(
        home_config_dir.join("config.toml"),
        r#"
[ai]
active_provider = "user-dashscope"

[ai.providers.user-dashscope]
type = "dashscope"
api_key = "sk-user"
model = "user-model"
"#,
    )
    .unwrap();
    std::fs::write(
        project_config_dir.join("config.toml"),
        r#"
[ai]
active_provider = "project-provider"
active_model = "project-model"

[ai.providers.project-provider]
type = "dashscope"
api_key = "sk-project"
"#,
    )
    .unwrap();

    let resp = run_registry_request_with_context(
        "auth",
        "state",
        Value::Null,
        home.path(),
        Some(project.path()),
    );
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["active_provider"], "user-dashscope");

    let saved = resp["data"]["saved_providers"].as_array().unwrap();
    assert_eq!(saved.len(), 1, "project provider must be ignored: {resp}");
    assert_eq!(saved[0]["provider_id"], "user-dashscope");
    assert_eq!(saved[0]["api_key_len"], 7);
    assert_eq!(saved[0]["model"], "user-model");
}

#[test]
fn registry_auth_configure_writes_home_config_only() {
    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    let home_config_dir = home.path().join(".copilot-shell");
    let project_config_dir = project.path().join(".copilot-shell");
    std::fs::create_dir_all(&home_config_dir).unwrap();
    std::fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("config.toml");
    std::fs::write(
        &project_config_path,
        r#"
[ai]
active_model = "project-model"

[ai.providers.project-provider]
type = "dashscope"
api_key = "sk-project"
"#,
    )
    .unwrap();

    let resp = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "home-provider",
            "provider_type": "dashscope",
            "values": {
                "api_key": "sk-home",
                "model": "home-model"
            }
        }),
        home.path(),
        Some(project.path()),
    );
    assert_eq!(resp["success"], true);

    let home_config = std::fs::read_to_string(home_config_dir.join("config.toml")).unwrap();
    let project_config = std::fs::read_to_string(project_config_path).unwrap();

    assert!(home_config.contains("[ai.providers.home-provider]"));
    assert!(home_config.contains("api_key = \"sk-home\""));
    assert!(!home_config.contains("project-model"));
    assert!(!home_config.contains("project-provider"));
    assert!(project_config.contains("project-model"));
    assert!(project_config.contains("sk-project"));
}

#[test]
fn registry_unknown_domain_returns_error() {
    let resp = run_registry_request("unknown_domain", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("unknown domain"));
}

#[test]
fn registry_unsupported_action_returns_error() {
    let resp = run_registry_request("extensions", "invalid_action", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["success"], false);
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("unsupported action"));
}

#[test]
fn registry_extensions_detail_nonexistent_returns_error() {
    let params = serde_json::json!({ "name": "nonexistent-extension-xyz" });
    let resp = run_registry_request("extensions", "detail", params);
    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("not found"));
}

#[test]
fn registry_skills_detail_nonexistent_returns_error() {
    let params = serde_json::json!({ "name": "nonexistent-skill-xyz" });
    let resp = run_registry_request("skills", "detail", params);
    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("not found"));
}
