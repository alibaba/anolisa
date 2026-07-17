use super::model::ShellEnvironmentObserver;
use super::osc::*;
use crate::types::{
    CommandOrigin, ShellEventKind, ShellHandoffRequest, COMMAND_OUTPUT_REF_MAX_BYTES,
    SESSION_OUTPUT_REF_MAX_BYTES,
};
use std::os::unix::fs::PermissionsExt;

const TEST_MARKER_TOKEN: &str = "test-marker-token";

#[test]
fn parser_clean_strips_zsh_bracketed_paste_and_applies_backspace() {
    let mut parser = parser_for_test("clean-zsh-control");
    let input =
        b"\x1b[0m\x1b[27m\x1b[24m\x1b[Jcosh-osc$ \x1b[K\x1b[?2004he\x08echo ok\x1b[?2004l\r\n";

    parser.feed(input).expect("feed");

    assert_eq!(
        String::from_utf8_lossy(&parser.clean),
        "cosh-osc$ echo ok\r\n"
    );
    assert_eq!(parser.display, input);
}

#[test]
fn parser_clean_handles_split_zsh_bracketed_paste_control() {
    let mut parser = parser_for_test("clean-zsh-split-control");

    parser.feed(b"\x1b[?20").expect("feed partial");
    assert!(parser.clean.is_empty());
    parser.feed(b"04hcmd\x1b[?2004l").expect("feed remainder");

    assert_eq!(String::from_utf8_lossy(&parser.clean), "cmd");
}

#[test]
fn precmd_count_tracks_shell_ready_and_command_events() {
    let mut parser = parser_for_test("precmd-count");
    assert_eq!(parser.precmd_count(), 0);

    let mut precmd_no_cmd: Vec<u8> = Vec::new();
    precmd_no_cmd.extend_from_slice(b"\x1b]1337;COSH;");
    precmd_no_cmd
        .extend_from_slice(br#"{"event":"precmd","token":"test-marker-token","cwd":"/tmp"}"#);
    precmd_no_cmd.push(b'\x07');
    parser.feed(&precmd_no_cmd).expect("feed precmd");
    assert_eq!(parser.precmd_count(), 1);

    let mut preexec: Vec<u8> = Vec::new();
    preexec.extend_from_slice(b"\x1b]1337;COSH;");
    preexec.extend_from_slice(
        br#"{"event":"preexec","token":"test-marker-token","command":"echo hi","cwd":"/tmp"}"#,
    );
    preexec.push(b'\x07');
    parser.feed(&preexec).expect("feed preexec");
    assert_eq!(parser.precmd_count(), 1);

    let mut precmd_ok: Vec<u8> = Vec::new();
    precmd_ok.extend_from_slice(b"\x1b]1337;COSH;");
    precmd_ok.extend_from_slice(
        br#"{"event":"precmd","token":"test-marker-token","status":0,"cwd":"/tmp"}"#,
    );
    precmd_ok.push(b'\x07');
    parser.feed(&precmd_ok).expect("feed precmd ok");
    assert_eq!(parser.precmd_count(), 2);

    let mut preexec2: Vec<u8> = Vec::new();
    preexec2.extend_from_slice(b"\x1b]1337;COSH;");
    preexec2.extend_from_slice(
        br#"{"event":"preexec","token":"test-marker-token","command":"false","cwd":"/tmp"}"#,
    );
    preexec2.push(b'\x07');
    parser.feed(&preexec2).expect("feed preexec2");

    let mut precmd_fail: Vec<u8> = Vec::new();
    precmd_fail.extend_from_slice(b"\x1b]1337;COSH;");
    precmd_fail.extend_from_slice(
        br#"{"event":"precmd","token":"test-marker-token","status":1,"cwd":"/tmp"}"#,
    );
    precmd_fail.push(b'\x07');
    parser.feed(&precmd_fail).expect("feed precmd fail");
    assert_eq!(parser.precmd_count(), 3);
}

#[test]
fn pending_handoff_origin_is_consumed_by_matching_preexec() {
    let mut parser = parser_for_test("origin-match");
    let request = ShellHandoffRequest::new(
        "echo hi".to_string(),
        "$ echo hi".to_string(),
        "user_analysis_action",
        "user",
        "approval-1".to_string(),
        "run-1".to_string(),
        1,
    )
    .expect("handoff request");
    parser.register_pending_handoff_origin(&request);

    feed_preexec(&mut parser, "echo hi");

    let event = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandStarted)
        .expect("command started");
    assert_eq!(
        event.command_origin,
        Some(CommandOrigin::UserAnalysisAction)
    );

    feed_precmd(&mut parser, 0);

    let event = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandCompleted)
        .expect("command completed");
    assert_eq!(
        event.command_origin,
        Some(CommandOrigin::UserAnalysisAction)
    );
}

#[test]
fn pending_handoff_origin_mismatch_becomes_unknown() {
    let mut parser = parser_for_test("origin-mismatch");
    let request = ShellHandoffRequest::new(
        "echo expected".to_string(),
        "$ echo expected".to_string(),
        "approved_provider_shell_tool",
        "user",
        "approval-1".to_string(),
        "run-1".to_string(),
        1,
    )
    .expect("handoff request");
    parser.register_pending_handoff_origin(&request);

    feed_preexec(&mut parser, "echo actual");

    let event = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandStarted)
        .expect("command started");
    assert_eq!(event.command_origin, Some(CommandOrigin::Unknown));
}

#[test]
fn trusted_preexec_path_reuses_and_advances_normalized_generation() {
    let mut parser = parser_for_test("path-generation");

    feed_environment_marker(
        &mut parser,
        "precmd",
        None,
        "/first:/first:relative:/second/",
        false,
        Some("path-generation"),
    );
    assert_eq!(
        parser
            .shell_environment_snapshot
            .as_ref()
            .unwrap()
            .generation,
        1
    );
    assert_eq!(
        parser
            .shell_environment_snapshot
            .as_ref()
            .unwrap()
            .marker_sequence,
        1
    );
    assert_eq!(
        parser.shell_environment_snapshot.as_ref().unwrap().path,
        "/first:/second"
    );

    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo one"),
        "/first:/second",
        true,
        Some("path-generation"),
    );
    let first = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandStarted)
        .expect("first command start");
    assert_eq!(first.shell_environment_generation, Some(1));
    assert_eq!(
        parser
            .shell_environment_snapshot
            .as_ref()
            .unwrap()
            .marker_sequence,
        2
    );
    feed_precmd(&mut parser, 0);
    let completed = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandCompleted)
        .expect("first command completion");
    assert_eq!(completed.shell_environment_generation, Some(1));

    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo two"),
        "/third:/second",
        true,
        Some("path-generation"),
    );
    let second = parser
        .events
        .iter()
        .filter(|event| event.kind == ShellEventKind::CommandStarted)
        .nth(1)
        .expect("second command start");
    assert_eq!(second.shell_environment_generation, Some(2));
    assert_eq!(
        parser
            .shell_environment_snapshot
            .as_ref()
            .unwrap()
            .marker_sequence,
        3
    );
}

#[test]
fn untrusted_or_invalid_environment_marker_never_binds_generation() {
    let mut parser = parser_for_test("path-untrusted");

    feed_environment_marker(
        &mut parser,
        "precmd",
        None,
        "/provisional",
        false,
        Some("path-untrusted"),
    );
    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo untrusted"),
        "/provisional",
        false,
        Some("path-untrusted"),
    );
    let untrusted = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandStarted)
        .expect("untrusted command start");
    assert_eq!(untrusted.shell_environment_generation, None);
    feed_precmd(&mut parser, 0);

    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo wrong-session"),
        "/wrong",
        true,
        Some("different-session"),
    );
    assert_eq!(
        parser
            .events
            .iter()
            .filter(|event| event.kind == ShellEventKind::CommandStarted)
            .count(),
        1
    );
    assert_eq!(
        parser
            .shell_environment_snapshot
            .as_ref()
            .unwrap()
            .marker_sequence,
        2
    );

    let oversized = format!("/{}", "x".repeat(8192));
    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo oversized"),
        &oversized,
        true,
        Some("path-untrusted"),
    );
    let oversized_start = parser
        .events
        .iter()
        .filter(|event| event.kind == ShellEventKind::CommandStarted)
        .nth(1)
        .expect("oversized command start");
    assert_eq!(oversized_start.shell_environment_generation, None);
    assert_eq!(
        parser
            .shell_environment_snapshot
            .as_ref()
            .unwrap()
            .marker_sequence,
        2
    );
}

#[test]
fn path_snapshot_accepts_exact_eight_kibibyte_boundary() {
    let mut parser = parser_for_test("path-eight-kib");
    let path = format!("/{}", "x".repeat(8191));

    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo boundary"),
        &path,
        true,
        Some("path-eight-kib"),
    );

    let start = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandStarted)
        .expect("boundary command start");
    assert_eq!(start.shell_environment_generation, Some(1));
    assert_eq!(
        parser.shell_environment_snapshot.as_ref().unwrap().path,
        path
    );
}

#[test]
fn environment_marker_with_wrong_token_does_not_update_state() {
    let mut parser = parser_for_test("path-wrong-token");
    let marker = serde_json::json!({
        "event": "preexec",
        "token": "wrong-token",
        "session_id": "path-wrong-token",
        "command": "echo forged",
        "cwd": "/tmp",
        "path": "/forged",
        "path_trusted": true,
        "status": 0,
    });
    let bytes = format!("\x1b]1337;COSH;{marker}\x07");

    parser.feed(bytes.as_bytes()).expect("feed forged marker");

    assert!(parser.shell_environment_snapshot.is_none());
    assert!(parser.events.is_empty());
}

#[test]
fn completion_keeps_generation_captured_at_command_start() {
    let mut parser = parser_for_test("path-completion-stable");
    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo stable"),
        "/at-start",
        true,
        Some("path-completion-stable"),
    );

    feed_environment_marker(
        &mut parser,
        "precmd",
        None,
        "/after-command",
        false,
        Some("path-completion-stable"),
    );

    let completed = parser
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandCompleted)
        .expect("completed command");
    assert_eq!(completed.shell_environment_generation, Some(1));
    assert_eq!(
        parser
            .shell_environment_snapshot
            .as_ref()
            .unwrap()
            .generation,
        2
    );
}

#[test]
fn accepted_environment_snapshots_are_forwarded_without_events_or_journal_fields() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut parser = parser_for_test("path-observer").with_environment_observer(
        ShellEnvironmentObserver::new(move |snapshot| {
            sender.send(snapshot).expect("forward snapshot");
        }),
    );

    feed_environment_marker(
        &mut parser,
        "precmd",
        None,
        "/provisional",
        false,
        Some("path-observer"),
    );
    feed_environment_marker(
        &mut parser,
        "preexec",
        Some("echo observed"),
        "/authoritative",
        true,
        Some("path-observer"),
    );

    let provisional = receiver.recv().expect("provisional snapshot");
    let authoritative = receiver.recv().expect("authoritative snapshot");
    assert_eq!(provisional.path, "/provisional");
    assert_eq!(authoritative.path, "/authoritative");
    assert!(parser.events.iter().all(|event| {
        serde_json::to_value(event)
            .expect("serialize event")
            .get("path")
            .is_none()
    }));
}

#[test]
fn parser_preserves_pending_handoff_command_echo_for_crlf() {
    let mut parser = parser_for_test("handoff-echo-crlf");
    let request = ShellHandoffRequest::new(
        "printf hi".to_string(),
        "$ printf hi".to_string(),
        "approved_provider_shell_tool",
        "user",
        "approval-1".to_string(),
        "run-1".to_string(),
        1,
    )
    .expect("handoff request");
    let mut echo = b"prompt$ ".to_vec();
    let mut command = request.pty_bytes().expect("handoff bytes");
    command.pop();
    echo.extend_from_slice(&command);
    echo.extend_from_slice(b"\r\nhi");

    parser.register_pending_handoff_origin(&request);
    parser.feed(&echo).expect("feed handoff echo");

    let display = String::from_utf8_lossy(&parser.display);
    assert_eq!(display, "prompt$ printf hi\r\nhi");
    let clean = String::from_utf8_lossy(&parser.clean);
    assert_eq!(clean, "prompt$ printf hi\r\nhi");
}

#[test]
fn parser_preserves_pending_handoff_command_echo_for_cr() {
    let mut parser = parser_for_test("handoff-echo-cr");
    let request = ShellHandoffRequest::new(
        "printf hi".to_string(),
        "$ printf hi".to_string(),
        "approved_provider_shell_tool",
        "user",
        "approval-1".to_string(),
        "run-1".to_string(),
        1,
    )
    .expect("handoff request");
    let mut echo = b"prompt$ ".to_vec();
    let mut command = request.pty_bytes().expect("handoff bytes");
    command.pop();
    echo.extend_from_slice(&command);
    echo.extend_from_slice(b"\x1b[?2004l\rhi");

    parser.register_pending_handoff_origin(&request);
    parser.feed(&echo).expect("feed handoff echo");

    let display = String::from_utf8_lossy(&parser.display);
    assert_eq!(display, "prompt$ printf hi\x1b[?2004l\rhi");
    let clean = String::from_utf8_lossy(&parser.clean);
    assert_eq!(clean, "prompt$ printf hi\rhi");
}

#[test]
fn output_ref_file_uses_private_permissions() {
    let dir =
        std::env::temp_dir().join(format!("cosh-shell-osc-output-ref-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let path = write_output_ref(&dir, "cmd-1", b"secret-ish\n").expect("write output ref");

    assert_eq!(
        std::fs::metadata(&dir)
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn output_ref_file_is_capped_but_preserves_head_and_tail() {
    let dir = std::env::temp_dir().join(format!(
        "cosh-shell-osc-output-ref-cap-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut output = Vec::new();
    output.extend_from_slice(b"head-line\n");
    output.extend(std::iter::repeat_n(b'x', COMMAND_OUTPUT_REF_MAX_BYTES));
    output.extend_from_slice(b"\ntail-line\n");

    let path = write_output_ref(&dir, "cmd-1", &output).expect("write output ref");
    let captured = std::fs::read(&path).expect("read output ref");
    let captured_text = String::from_utf8(captured.clone()).expect("utf8 capped output");

    assert!(captured.len() <= COMMAND_OUTPUT_REF_MAX_BYTES);
    assert!(captured_text.starts_with("head-line"), "{captured_text}");
    assert!(
        captured_text.contains("[captured output truncated:"),
        "{captured_text}"
    );
    assert!(captured_text.ends_with("tail-line\n"), "{captured_text}");
    assert!(
        captured_text.contains(&format!("original_bytes={}", output.len())),
        "{captured_text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capped_output_ref_respects_utf8_boundaries() {
    let input = "头".repeat(COMMAND_OUTPUT_REF_MAX_BYTES / 3 + 10);

    let captured = capped_output_ref_bytes(input.as_bytes(), 4096);

    let captured_text = String::from_utf8(captured).expect("valid utf8");
    assert!(captured_text.contains("[captured output truncated:"));
    assert!(captured_text.starts_with('头'));
    assert!(captured_text.ends_with('头'));
}

#[test]
fn output_ref_session_cap_marks_later_output_unavailable() {
    let dir = std::env::temp_dir().join(format!(
        "cosh-shell-osc-output-ref-session-cap-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);

    let first =
        write_output_ref_with_session_cap(&dir, "cmd-1", b"12345", 0, 8).expect("first ref");
    let second = write_output_ref_with_session_cap(&dir, "cmd-2", b"6789", first.captured_bytes, 8)
        .expect("second ref");

    assert_eq!(first.status, OutputRefCaptureStatus::Captured);
    assert!(first.path.as_ref().is_some_and(|path| path.exists()));
    assert_eq!(second.status, OutputRefCaptureStatus::SessionCapReached);
    assert!(second.path.is_none());
    assert!(!dir.join("cmd-2.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parser_session_cap_preserves_command_facts_without_output_ref() {
    let mut parser = parser_for_test("session-cap-events");
    parser.captured_output_ref_bytes = SESSION_OUTPUT_REF_MAX_BYTES;

    let mut preexec: Vec<u8> = Vec::new();
    preexec.extend_from_slice(b"\x1b]1337;COSH;");
    preexec.extend_from_slice(
        br#"{"event":"preexec","token":"test-marker-token","command":"printf capped","cwd":"/tmp","timestamp_ms":10}"#,
    );
    preexec.push(b'\x07');
    parser.feed(&preexec).expect("feed preexec");
    parser.feed(b"captured body\n").expect("feed output");

    let mut precmd: Vec<u8> = Vec::new();
    precmd.extend_from_slice(b"\x1b]1337;COSH;");
    precmd.extend_from_slice(
        br#"{"event":"precmd","token":"test-marker-token","status":0,"cwd":"/tmp","timestamp_ms":20}"#,
    );
    precmd.push(b'\x07');
    parser.feed(&precmd).expect("feed precmd");

    let event = parser
        .events
        .iter()
        .find(|event| {
            matches!(
                event.kind,
                ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed
            ) && event.command_id.as_deref() == Some("cmd-1")
        })
        .expect("finished command event");
    assert_eq!(event.command.as_deref(), Some("printf capped"));
    assert_eq!(event.terminal_output_ref, None);
    assert_eq!(
        event.terminal_output_bytes,
        Some("captured body\n".len() as u64)
    );
    assert_eq!(event.component.as_deref(), Some("output_capture"));
    assert_eq!(
        event.message.as_deref(),
        Some("output_capture_status: unavailable; reason: session_output_cap_reached")
    );
}

fn parser_for_test(name: &str) -> OscParser {
    let dir =
        std::env::temp_dir().join(format!("cosh-shell-osc-test-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("output ref dir");
    OscParser::new(name.to_string(), dir, TEST_MARKER_TOKEN.to_string())
}

fn feed_preexec(parser: &mut OscParser, command: &str) {
    let marker = format!(
        "\x1b]1337;COSH;{{\"event\":\"preexec\",\"token\":\"test-marker-token\",\"command\":{command_json},\"cwd\":\"/tmp\"}}\x07",
        command_json = serde_json::to_string(command).expect("command json")
    );
    parser.feed(marker.as_bytes()).expect("feed preexec");
}

fn feed_precmd(parser: &mut OscParser, status: i32) {
    let marker = format!(
        "\x1b]1337;COSH;{{\"event\":\"precmd\",\"token\":\"test-marker-token\",\"status\":{status},\"cwd\":\"/tmp\"}}\x07"
    );
    parser.feed(marker.as_bytes()).expect("feed precmd");
}

fn feed_environment_marker(
    parser: &mut OscParser,
    event: &str,
    command: Option<&str>,
    path: &str,
    path_trusted: bool,
    session_id: Option<&str>,
) {
    let marker = serde_json::json!({
        "event": event,
        "token": TEST_MARKER_TOKEN,
        "session_id": session_id,
        "command": command,
        "cwd": "/tmp",
        "path": path,
        "path_trusted": path_trusted,
        "status": 0,
    });
    let bytes = format!("\x1b]1337;COSH;{marker}\x07");
    parser
        .feed(bytes.as_bytes())
        .expect("feed environment marker");
}
