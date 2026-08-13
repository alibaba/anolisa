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

fn permission_adapter(directory: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let path = directory.path().join("codex-acp");
    let response = directory.path().join("permission-response.json");
    let script = r#"#!/bin/sh
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1)
            printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{}}}'
            ;;
        2)
            printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"permission-session"}}'
            ;;
        3)
            printf '%s\n' '{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"sessionId":"permission-session","toolCall":{"toolCallId":"private-tool-id","title":"Run private operation","rawInput":{"token":"credential-secret"}},"options":[{"optionId":"allow","name":"Allow once","kind":"allow_once"},{"optionId":"always","name":"Allow always","kind":"allow_always"},{"optionId":"reject","name":"Reject once","kind":"reject_once"}]}}'
            ;;
        4)
            printf '%s\n' "$line" > '__RESPONSE__'
            printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-3","result":{"stopReason":"end_turn"}}'
            ;;
    esac
done
"#
    .replace("__RESPONSE__", response.to_str().unwrap());
    fs::write(&path, script).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    (path, response)
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

#[test]
fn noninteractive_permission_cancels_and_persists_only_digests() {
    let workspace = tempfile::tempdir().unwrap();
    let (adapter, response) = permission_adapter(&workspace);
    let evidence = workspace.path().join("permission-evidence.jsonl");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cosh-gateway"))
        .args([
            "run",
            "--adapter",
            adapter.to_str().unwrap(),
            "--workspace",
            workspace.path().to_str().unwrap(),
            "--output",
            "jsonl",
            "--permission",
            "deny",
            "--permission-evidence",
            evidence.to_str().unwrap(),
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
        .write_all(b"private prompt\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"event\":\"permission_decided\""));
    assert!(stdout.contains("\"decision\":\"cancelled\""));
    assert!(!stdout.contains("private-tool-id"));
    assert!(!stdout.contains("credential-secret"));

    let stored = fs::read_to_string(evidence).unwrap();
    assert!(stored.contains("\"decision\":\"cancelled\""));
    assert!(stored.contains("\"workspace_digest\""));
    assert!(!stored.contains("permission-session"));
    assert!(!stored.contains("private-tool-id"));
    assert!(!stored.contains("credential-secret"));
    let answer = fs::read_to_string(response).unwrap();
    assert!(answer.contains("\"id\":99"));
    assert!(answer.contains("cancelled"));
}

#[test]
fn relative_permission_evidence_path_fails_before_adapter_launch() {
    let workspace = tempfile::tempdir().unwrap();
    let adapter = workspace.path().join("codex-acp");
    fs::write(&adapter, "#!/bin/sh\ntouch launched\n").unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cosh-gateway"))
        .current_dir(workspace.path())
        .args([
            "run",
            "--adapter",
            adapter.to_str().unwrap(),
            "--workspace",
            workspace.path().to_str().unwrap(),
            "--permission-evidence",
            "relative.jsonl",
        ])
        .stdin(Stdio::piped())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(12));
    assert!(!workspace.path().join("launched").exists());
    assert!(!workspace.path().join("relative.jsonl").exists());
}

#[test]
fn task_cli_rejects_invalid_identity_before_socket_io() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("absent.sock");
    let output = Command::new(env!("CARGO_BIN_EXE_cosh-gateway"))
        .args([
            "task",
            "--socket",
            socket.to_str().unwrap(),
            "--output",
            "jsonl",
            "get",
            "run_00000000-0000-0000-0000-000000000000",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(10));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"code\":\"invalid_request\""));
    assert!(stdout.contains("identifier prefix must be `tsk`"));
}

#[test]
fn serve_rejects_an_invalid_provisioned_installation_identity() {
    let output = Command::new(env!("CARGO_BIN_EXE_cosh-gateway"))
        .args(["serve", "--installation-id", "invalid"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(12));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("identifier prefix must be `ins`"));
}
