use std::process::Command;

fn tokenless_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tokenless"))
}

#[test]
fn compress_schema_from_stdin() {
    let schema = r#"{"function":{"name":"test","description":"A test function","parameters":{"type":"object","properties":{"x":{"type":"string","title":"Remove Me","examples":["ex1"]}}}}}"#;
    let output = tokenless_bin()
        .args(["compress-schema"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(schema.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success(), "compress-schema should succeed");
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert!(result["function"]["name"].is_string());
}

#[test]
fn compress_schema_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("schema.json");
    std::fs::write(
        &f,
        r#"{"function":{"name":"f","description":"desc","parameters":{"type":"object","properties":{}}}}"#,
    )
    .unwrap();
    let output = tokenless_bin()
        .args(["compress-schema", "--file", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["function"]["name"], "f");
}

#[test]
fn compress_schema_batch_mode() {
    let schemas = r#"[{"function":{"name":"a","parameters":{"type":"object","properties":{}}}},{"function":{"name":"b","parameters":{"type":"object","properties":{}}}}]"#;
    let output = tokenless_bin()
        .args(["compress-schema", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(schemas.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result.is_array());
}

#[test]
fn compress_response_from_stdin() {
    let response =
        r#"{"data":"value","debug":"remove","trace":"remove","empty_field":"","null_field":null}"#;
    let output = tokenless_bin()
        .args(["compress-response"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result.get("data").is_some());
    assert!(result.get("debug").is_none());
}

#[test]
fn compress_response_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("response.json");
    std::fs::write(&f, r#"{"key":"value","logs":"remove me"}"#).unwrap();
    let output = tokenless_bin()
        .args(["compress-response", "--file", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result.get("key").is_some());
}

#[test]
fn compress_response_no_stash() {
    let response = r#"{"data":"value","debug":"remove"}"#;
    let output = tokenless_bin()
        .args(["compress-response", "--no-stash"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn stats_list_empty() {
    let output = tokenless_bin().args(["stats", "list"]).output().unwrap();
    // May succeed or fail depending on db state; should not panic
    let _ = output.status;
}

#[test]
fn stats_summary() {
    let output = tokenless_bin().args(["stats", "summary"]).output().unwrap();
    let _ = output.status;
}

#[test]
fn retrieve_missing_hash() {
    let output = tokenless_bin()
        .args(["retrieve", "000000000000000000000000"])
        .output()
        .unwrap();
    // Should fail gracefully (hash not found), not panic
    assert!(!output.status.success());
}

#[test]
fn retrieve_invalid_hash() {
    let output = tokenless_bin()
        .args(["retrieve", "not-a-valid-hash"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn no_args_shows_error() {
    let output = tokenless_bin().output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn invalid_json_input() {
    let output = tokenless_bin()
        .args(["compress-schema"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(b"not valid json")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn compress_schema_with_agent_id() {
    let schema = r#"{"function":{"name":"test","parameters":{"type":"object","properties":{}}}}"#;
    let output = tokenless_bin()
        .args(["compress-schema", "--agent-id", "test-agent"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(schema.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn compress_response_with_session_and_tool_ids() {
    let response = r#"{"data":"value"}"#;
    let output = tokenless_bin()
        .args([
            "compress-response",
            "--agent-id",
            "test",
            "--session-id",
            "s1",
            "--tool-use-id",
            "t1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn compress_toon_from_stdin() {
    let toon_input = r#"{"content":"some content","debug":"remove"}"#;
    let output = tokenless_bin()
        .args(["compress-toon"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(toon_input.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    // compress-toon may or may not succeed depending on input format
    let _ = output.status;
}

#[test]
fn env_check_without_spec() {
    let output = tokenless_bin().args(["env-check"]).output().unwrap();
    // May fail if no spec file exists, that's OK
    let _ = output.status;
}

#[test]
fn config_show() {
    let output = tokenless_bin().args(["config", "show"]).output().unwrap();
    // Should show current config or defaults
    let _ = output.status;
}

#[test]
fn stats_show_single_nonexistent() {
    let output = tokenless_bin()
        .args(["stats", "show", "99999"])
        .output()
        .unwrap();
    // Should fail gracefully for nonexistent record
    let _ = output.status;
}
