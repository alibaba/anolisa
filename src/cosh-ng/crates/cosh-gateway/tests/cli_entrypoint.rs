//! Installed ACP entrypoint behavior over a deterministic local fake adapter.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

fn fake_adapter(directory: &tempfile::TempDir) -> std::path::PathBuf {
    let path = directory.path().join("codex-acp");
    fs::write(
        &path,
        r#"#!/bin/sh
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1)
            printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"entrypoint-fake","version":"1.0"}}}'
            ;;
        2)
            printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"entrypoint-session"}}'
            ;;
        3)
            printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"entrypoint-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"safe\u001b[2Jtext"}}}}'
            printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-3","result":{"stopReason":"end_turn"}}'
            ;;
    esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

#[test]
fn doctor_initializes_installed_adapter_without_prompting() {
    let workspace = tempfile::tempdir().unwrap();
    let adapter = fake_adapter(&workspace);
    let output = Command::new(env!("CARGO_BIN_EXE_cosh-gateway"))
        .args([
            "doctor",
            "--adapter",
            adapter.to_str().unwrap(),
            "--workspace",
            workspace.path().to_str().unwrap(),
            "--output",
            "jsonl",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout
        .lines()
        .any(|line| line.contains("\"event\":\"initialized\"")));
    assert!(stdout
        .lines()
        .any(|line| line.contains("\"event\":\"session_opened\"")));
    assert!(stdout
        .lines()
        .any(|line| line.contains("\"event\":\"doctor_ok\"")));
    assert!(!stdout.contains("session_update"));
}

#[test]
fn run_reads_prompt_from_stdin_and_escapes_terminal_controls() {
    let workspace = tempfile::tempdir().unwrap();
    let adapter = fake_adapter(&workspace);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cosh-gateway"))
        .args([
            "run",
            "--adapter",
            adapter.to_str().unwrap(),
            "--workspace",
            workspace.path().to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"inspect safely\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("safe\\u{1b}[2Jtext"));
    assert!(!stdout.as_bytes().contains(&0x1b));
    assert!(!stdout.contains("sessionUpdate"));
}

#[test]
fn missing_adapter_has_stable_profile_exit_and_jsonl_error() {
    let workspace = tempfile::tempdir().unwrap();
    let adapter = workspace.path().join("codex-acp");
    let output = Command::new(env!("CARGO_BIN_EXE_cosh-gateway"))
        .args([
            "doctor",
            "--adapter",
            adapter.to_str().unwrap(),
            "--workspace",
            workspace.path().to_str().unwrap(),
            "--output",
            "jsonl",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(11));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"event\":\"error\""));
    assert!(stdout.contains("\"code\":\"profile_invalid\""));
}
