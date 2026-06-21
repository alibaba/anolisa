use std::io::Write;
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
    let bin = binary_path();
    let home = tempfile::tempdir().expect("temp home");
    let request = serde_json::json!({
        "type": "registry_request",
        "request_id": "test-1",
        "domain": domain,
        "action": action,
        "params": params,
    });

    let mut child = Command::new(&bin)
        .arg("--registry")
        .env("HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
    assert!(resp["error"].as_str().unwrap().contains("unsupported action"));
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
