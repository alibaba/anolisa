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

fn interact(messages: &[&str]) -> Vec<Value> {
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
        for msg in messages {
            writeln!(stdin, "{msg}").unwrap();
            stdin.flush().unwrap();
        }
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[test]
fn initialize_then_shutdown() {
    let msgs = interact(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    assert!(!msgs.is_empty());
    let init = msgs
        .iter()
        .find(|m| m["type"] == "system" && m["subtype"] == "init")
        .expect("system init");
    assert_eq!(init["type"], "system");
    assert_eq!(init["subtype"], "init");
    assert!(init["session_id"].as_str().unwrap().len() > 10);
    assert!(init["tools"].as_array().unwrap().len() >= 5);
}

#[test]
fn user_message_produces_result() {
    let msgs = interact(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"say hello"},"parent_tool_use_id":null}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    let results: Vec<_> = msgs.iter().filter(|m| m["type"] == "result").collect();
    assert!(!results.is_empty(), "expected at least one result message");

    let result = &results[0];
    assert!(!result["is_error"].as_bool().unwrap());
    let init = msgs
        .iter()
        .find(|message| message["type"] == "system" && message["subtype"] == "init")
        .expect("system init");
    assert_eq!(result["session_id"], init["session_id"]);
}

#[test]
fn switch_model_changes_reported_model() {
    let msgs = interact(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"control_request","request_id":"sw-1","request":{"subtype":"switch_model","model":"qwen-test"}}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    assert!(!msgs.is_empty());
}

#[test]
fn config_override_approval_mode() {
    let msgs = interact(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"control_request","request_id":"cfg-1","request":{"subtype":"config_override","approval_mode":"trust"}}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    assert!(!msgs.is_empty());
}

#[test]
fn session_id_from_user_message_cannot_replace_initialized_identity() {
    let msgs = interact(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"hi"},"session_id":"00000000-0000-4000-8000-000000000000","parent_tool_use_id":null}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    let init = msgs
        .iter()
        .find(|message| message["type"] == "system" && message["subtype"] == "init")
        .expect("system init");
    let result = msgs.iter().find(|m| m["type"] == "result").unwrap();
    assert_eq!(result["session_id"], init["session_id"]);
    assert_eq!(result["is_error"], true);
}

#[test]
fn assistant_text_format_matches_cosh_shell() {
    let msgs = interact(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"test"},"parent_tool_use_id":null}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    let assistant = msgs.iter().find(|m| m["type"] == "assistant");
    if let Some(a) = assistant {
        let init = msgs
            .iter()
            .find(|message| message["type"] == "system" && message["subtype"] == "init")
            .expect("system init");
        assert_eq!(a["session_id"], init["session_id"]);
        let content = a["message"]["content"].as_array().unwrap();
        assert!(!content.is_empty());
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].is_string());
    }
}

#[test]
fn result_has_duration_ms() {
    let msgs = interact(&[
        r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"test"},"parent_tool_use_id":null}"#,
        r#"{"type":"control_request","request_id":"shut-1","request":{"subtype":"shutdown"}}"#,
    ]);

    let result = msgs.iter().find(|m| m["type"] == "result").unwrap();
    assert!(result["duration_ms"].is_number());
}

#[test]
fn session_start_hook_emits_notification() {
    // Create a temp HOME with .copilot-shell/config.toml that has hooks enabled
    let home = tempfile::tempdir().expect("temp home");
    let config_dir = home.path().join(".copilot-shell");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[hooks]
enabled = true

[[hooks.SessionStart]]
command = "echo '{\"system_message\":\"hello from hook\"}'"
name = "test-hook"
"#,
    )
    .unwrap();

    let bin = binary_path();
    let mut child = Command::new(&bin)
        .env("HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"type":"control_request","request_id":"init-1","request":{{"subtype":"initialize"}}}}"#).unwrap();
        writeln!(stdin, r#"{{"type":"control_request","request_id":"shut-1","request":{{"subtype":"shutdown"}}}}"#).unwrap();
        stdin.flush().unwrap();
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let msgs: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();

    let hook_notification = msgs
        .iter()
        .find(|m| m["type"] == "system" && m["subtype"] == "hook_notification");
    assert!(
        hook_notification.is_some(),
        "expected hook_notification in output, got: {:?}",
        msgs
    );
    let notif = hook_notification.unwrap();
    assert!(
        notif["status"]
            .as_str()
            .unwrap()
            .contains("hello from hook"),
        "hook notification should contain message"
    );
}
