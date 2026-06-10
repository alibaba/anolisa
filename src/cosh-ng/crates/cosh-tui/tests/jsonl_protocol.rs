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
    path.push("cosh-tui-core");
    path
}

fn run_with_input(lines: &[&str]) -> Vec<Value> {
    let bin = binary_path();
    let mut child = Command::new(&bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn {}: {e}", bin.display()));

    {
        let stdin = child.stdin.as_mut().unwrap();
        for line in lines {
            writeln!(stdin, "{line}").unwrap();
        }
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|e| panic!("bad JSON: {e}: {l}")))
        .collect()
}

#[test]
fn initialize_returns_system_init() {
    let msgs = run_with_input(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    assert!(!msgs.is_empty(), "expected at least one output message");
    let first = &msgs[0];
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "init");
    assert!(first["session_id"].is_string());
    assert!(first["model"].is_string());
    assert!(first["tools"].is_array());
}

#[test]
fn user_message_returns_assistant_and_result() {
    let msgs = run_with_input(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"hello"},"session_id":"test-sess","parent_tool_use_id":null}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    assert!(msgs.len() >= 2, "expected at least 2 messages, got {}", msgs.len());

    let init = &msgs[0];
    assert_eq!(init["type"], "system");

    let has_result = msgs.iter().any(|m| m["type"] == "result");
    assert!(has_result, "expected a result message");

    let result = msgs.iter().find(|m| m["type"] == "result").unwrap();
    assert_eq!(result["session_id"], "test-sess");
}

#[test]
fn shutdown_terminates_process() {
    let msgs = run_with_input(&[
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    assert!(msgs.is_empty() || msgs.iter().all(|m| m["type"] != "result"));
}

#[test]
fn output_format_matches_cosh_shell_expectations() {
    let msgs = run_with_input(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    let init = &msgs[0];

    assert!(
        init.get("session_id").is_some(),
        "system init must have top-level session_id"
    );
    assert!(
        init.get("model").is_some(),
        "system init must have top-level model"
    );
    assert!(
        init.get("tools").is_some(),
        "system init must have top-level tools"
    );
    assert_eq!(
        init.get("type").unwrap().as_str().unwrap(),
        "system"
    );
    assert_eq!(
        init.get("subtype").unwrap().as_str().unwrap(),
        "init"
    );
}
