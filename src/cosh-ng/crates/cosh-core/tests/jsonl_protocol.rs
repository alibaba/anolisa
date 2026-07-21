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

fn run_with_input(lines: &[&str]) -> Vec<Value> {
    let bin = binary_path();
    let home = tempfile::tempdir().expect("temp home");
    let mut child = Command::new(&bin)
        .env("HOME", home.path())
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
    let capability = msgs
        .iter()
        .find(|m| m["type"] == "control_response")
        .expect("initialize capability response");
    assert_eq!(
        capability["response"]["response"]["capabilities"]
            ["can_handle_host_executed_shell_tool_result"],
        true
    );

    let init = msgs
        .iter()
        .find(|m| m["type"] == "system" && m["subtype"] == "init")
        .expect("system init");
    assert!(init["session_id"].is_string());
    assert!(init["model"].is_string());
    assert!(init["tools"].is_array());
}

#[test]
fn user_message_returns_assistant_and_result() {
    let msgs = run_with_input(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"hello"},"parent_tool_use_id":null}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    assert!(
        msgs.len() >= 2,
        "expected at least 2 messages, got {}",
        msgs.len()
    );

    assert!(
        msgs.iter()
            .any(|m| m["type"] == "system" && m["subtype"] == "init"),
        "expected system init"
    );

    let has_result = msgs.iter().any(|m| m["type"] == "result");
    assert!(has_result, "expected a result message");

    let init = msgs
        .iter()
        .find(|m| m["type"] == "system" && m["subtype"] == "init")
        .unwrap();
    let result = msgs.iter().find(|m| m["type"] == "result").unwrap();
    assert_eq!(result["session_id"], init["session_id"]);
}

#[test]
fn user_message_cannot_replace_initialized_session_id() {
    let msgs = run_with_input(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"hello"},"session_id":"default","parent_tool_use_id":null}"#,
        r#"{"type":"user","message":{"role":"user","content":"replace"},"session_id":"00000000-0000-4000-8000-000000000000","parent_tool_use_id":null}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    let init = msgs
        .iter()
        .find(|message| message["type"] == "system" && message["subtype"] == "init")
        .expect("system init");
    let results = msgs
        .iter()
        .filter(|message| message["type"] == "result")
        .collect::<Vec<_>>();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["session_id"], init["session_id"]);
    assert_eq!(results[1]["session_id"], init["session_id"]);
    assert_eq!(results[1]["is_error"], true);
    assert!(results[1]["result"]
        .as_str()
        .is_some_and(|value| value.contains("session identity conflict")));
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

    let init = msgs
        .iter()
        .find(|m| m["type"] == "system" && m["subtype"] == "init")
        .expect("system init");

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
    assert_eq!(init.get("type").unwrap().as_str().unwrap(), "system");
    assert_eq!(init.get("subtype").unwrap().as_str().unwrap(), "init");
}

#[test]
fn invalid_jsonl_input_returns_error_and_fails() {
    let bin = binary_path();
    let home = tempfile::tempdir().expect("temp home");
    let mut child = Command::new(&bin)
        .env("HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cosh-core");

    const SECRET_INPUT: &str = "token=must-not-echo";
    writeln!(child.stdin.as_mut().expect("stdin"), "{SECRET_INPUT}").expect("write invalid input");
    let output = child.wait_with_output().expect("wait for cosh-core");
    assert!(!output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(SECRET_INPUT),
        "invalid input must not be echoed"
    );
    let messages = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("valid JSONL output"))
        .collect::<Vec<_>>();
    let error = messages
        .iter()
        .find(|message| message["type"] == "result" && message["is_error"] == true)
        .expect("invalid input error result");
    assert_eq!(error["subtype"], "error");
    assert_eq!(error["error_code"], "InvalidJsonlInput");
    assert_eq!(error["errors"][0], "failed to parse stdin line as JSON");
}
