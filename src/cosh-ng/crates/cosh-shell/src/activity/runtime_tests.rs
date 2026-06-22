use super::runtime::*;
use crate::runtime::prelude::*;
use std::os::unix::fs::PermissionsExt;

fn governed(event: AgentEvent) -> GovernedEvent {
    GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event,
        reason: "test".to_string(),
        display_text: "test".to_string(),
        auto_execute: false,
    }
}

#[test]
fn activity_tool_output_summary_uses_state_language() {
    let mut state = InlineState {
        language: Language::ZhCn,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolOutputDelta {
            run_id: "run-1".to_string(),
            tool_id: "tool-1".to_string(),
            stream: "stdout".to_string(),
            text: "line 1\nline 2".to_string(),
        })],
    );

    assert_eq!(ids, vec!["out-1"]);
    let row = state
        .activity
        .rows
        .iter()
        .find(|row| row.id == "out-1")
        .expect("activity row");
    assert_eq!(row.summary, "stdout 已捕获；[Details] out-1");
    assert!(row.detail.contains("stream: stdout"));

    let mut output = Vec::new();
    render_activity_details_by_id(&state, "out-1", &mut output)
        .expect("details result")
        .expect("render details");
    let output = String::from_utf8(output).expect("utf8 output");
    assert!(output.contains("活动详情 out-1"), "{output}");
    assert!(output.contains("运行: run-1"), "{output}");
    assert!(output.contains("详情:"), "{output}");
    assert!(output.contains("stream: stdout"), "{output}");
}

#[test]
fn activity_tool_output_details_hide_internal_output_ref_path() {
    let dir = std::env::temp_dir().join(format!(
        "cosh-shell-activity-details-hide-ref-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut state = InlineState::with_raw_session_dir(&dir);
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolOutputDelta {
            run_id: "run-1".to_string(),
            tool_id: "tool-1".to_string(),
            stream: "stdout".to_string(),
            text: "secret-ish\n".to_string(),
        })],
    );

    assert_eq!(ids, vec!["out-1"]);
    let output_ref = dir.join("agent-output-refs/out-1.txt");
    assert!(output_ref.exists(), "output ref should still be captured");

    let mut output = Vec::new();
    render_activity_details_by_id(&state, "out-1", &mut output)
        .expect("details result")
        .expect("render details");
    let output = String::from_utf8(output).expect("utf8 output");
    assert!(output.contains("capture_status: captured"), "{output}");
    assert!(output.contains("output_ref: <hidden>"), "{output}");
    assert!(!output.contains(output_ref.to_str().unwrap()), "{output}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn activity_tool_output_details_show_internal_output_ref_in_debug() {
    let dir = std::env::temp_dir().join(format!(
        "cosh-shell-activity-details-debug-ref-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut state = InlineState {
        debug: true,
        ..InlineState::with_raw_session_dir(&dir)
    };
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolOutputDelta {
            run_id: "run-1".to_string(),
            tool_id: "tool-1".to_string(),
            stream: "stdout".to_string(),
            text: "debug-visible\n".to_string(),
        })],
    );

    assert_eq!(ids, vec!["out-1"]);
    let output_ref = dir.join("agent-output-refs/out-1.txt");
    assert!(output_ref.exists(), "output ref should be captured");

    let mut output = Vec::new();
    render_activity_details_by_id(&state, "out-1", &mut output)
        .expect("details result")
        .expect("render details");
    let output = String::from_utf8(output).expect("utf8 output");
    assert!(output.contains("debug_output_ref:"), "{output}");
    assert!(output.contains("out-1.txt"), "{output}");
    assert!(output.contains("output_ref: <hidden>"), "{output}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn non_shell_provider_tool_call_renders_activity_card() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: None,
            name: "Read".to_string(),
            input: r#"{"file_path":"Cargo.toml"}"#.to_string(),
        })],
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(output.contains("Activity"), "{output}");
    assert!(
        output.contains("Read called: Cargo.toml; [Details] tool-1"),
        "{output}"
    );
}

#[test]
fn shell_provider_tool_call_still_uses_shell_visibility_path() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: None,
            name: "run_shell_command".to_string(),
            input: "df -h".to_string(),
        })],
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(!output.contains("Activity"), "{output}");
    assert!(!output.contains("run_shell_command called"), "{output}");
}

#[test]
fn provider_native_shell_output_renders_transcript_without_activity_card() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-1".to_string()),
                name: "run_shell_command".to_string(),
                input: serde_json::json!({ "command": "df -h" }).to_string(),
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "toolu-1".to_string(),
                stream: "stdout".to_string(),
                text: "Filesystem\n/dev/disk1\n".to_string(),
            }),
            governed(AgentEvent::ToolCompleted {
                run_id: "run-1".to_string(),
                tool_id: "toolu-1".to_string(),
                status: "completed".to_string(),
            }),
        ],
    );

    let mut output = Vec::new();
    render_provider_native_shell_transcript(&mut state, &ids, &mut output)
        .expect("render shell transcript");
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
    assert!(!output.contains("Activity"), "{output}");
    assert!(
        !output.contains("stdout captured; [Details] out-1"),
        "{output}"
    );
    assert!(!output.contains("Tool completed"), "{output}");
    let detail = &state
        .activity
        .rows
        .iter()
        .find(|row| row.id == "out-1")
        .expect("output row")
        .detail;
    assert!(
        detail.contains("provider_native_shell_command: df -h"),
        "{detail}"
    );
}

#[test]
fn provider_native_shell_transcript_uses_structured_tool_state() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-1".to_string()),
                name: "run_shell_command".to_string(),
                input: serde_json::json!({ "command": "df -h" }).to_string(),
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "toolu-1".to_string(),
                stream: "stdout".to_string(),
                text: "Filesystem\n/dev/disk1\n".to_string(),
            }),
        ],
    );
    let row = state
        .activity
        .rows
        .iter_mut()
        .find(|row| row.id == "out-1")
        .expect("output row");
    row.detail =
        "tool: toolu-1\nstream: stdout\noutput_ref: <hidden>\nDETAIL_ONLY_SHOULD_NOT_RENDER\n"
            .to_string();

    let mut output = Vec::new();
    render_provider_native_shell_transcript(&mut state, &ids, &mut output)
        .expect("render shell transcript");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
    assert!(
        !output.contains("DETAIL_ONLY_SHOULD_NOT_RENDER"),
        "{output}"
    );
}

#[test]
fn provider_native_streamed_shell_output_renders_transcript_without_control_permission() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-shell".to_string()),
                name: "run_shell_command".to_string(),
                input: "df -h".to_string(),
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "toolu-shell".to_string(),
                stream: "stdout".to_string(),
                text: "Filesystem\n/dev/disk1\n".to_string(),
            }),
            governed(AgentEvent::ToolCompleted {
                run_id: "run-1".to_string(),
                tool_id: "toolu-shell".to_string(),
                status: "success".to_string(),
            }),
        ],
    );

    let mut output = Vec::new();
    render_provider_native_shell_transcript(&mut state, &ids, &mut output)
        .expect("render shell transcript");
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
    assert!(!output.contains("Activity"), "{output}");
    assert!(
        !output.contains("stdout captured; [Details] out-1"),
        "{output}"
    );
    let detail = &state
        .activity
        .rows
        .iter()
        .find(|row| row.id == "out-1")
        .expect("output row")
        .detail;
    assert!(
        detail.contains("provider_native_shell_command: df -h"),
        "{detail}"
    );
}

#[test]
fn control_protocol_policy_suppresses_provider_auto_approved_shell_activity() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows_with_policy(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-shell".to_string()),
                name: "run_shell_command".to_string(),
                input: "df -h".to_string(),
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "toolu-shell".to_string(),
                stream: "stdout".to_string(),
                text: "Filesystem\n/dev/disk1\n".to_string(),
            }),
            governed(AgentEvent::ToolCompleted {
                run_id: "run-1".to_string(),
                tool_id: "toolu-shell".to_string(),
                status: "success".to_string(),
            }),
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-read".to_string()),
                name: "Read".to_string(),
                input: r#"{"file_path":"Cargo.toml"}"#.to_string(),
            }),
        ],
        ActivityRecordPolicy {
            suppress_provider_native_shell: true,
        },
    );

    let mut output = Vec::new();
    render_provider_native_shell_transcript(&mut state, &ids, &mut output)
        .expect("render shell transcript");
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
    assert!(
        !output.contains("run_shell_command auto-approved by provider"),
        "{output}"
    );
    assert!(
        output.contains("Read called: Cargo.toml; [Details]"),
        "{output}"
    );
    assert!(state.activity.rows.iter().any(|row| {
        row.detail.contains("evidence: ProviderNativeShellBypass")
            && row
                .detail
                .contains("provider_native_shell_bypassed_control_protocol")
            && row
                .detail
                .contains("provider_auto_approval_status: auto_approved_by_provider")
            && row.detail.contains("provider_native_shell_command: df -h")
    }));
}

#[test]
fn debug_mode_keeps_provider_auto_approved_shell_activity() {
    let mut state = InlineState {
        language: Language::EnUs,
        debug: true,
        ..InlineState::default()
    };
    let ids = record_activity_rows_with_policy(
        &mut state,
        &[governed(AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: Some("toolu-shell".to_string()),
            name: "run_shell_command".to_string(),
            input: "df -h".to_string(),
        })],
        ActivityRecordPolicy {
            suppress_provider_native_shell: true,
        },
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(
        output.contains("run_shell_command auto-approved by provider: $ df -h; [Details]"),
        "{output}"
    );
}

#[test]
fn question_tool_call_is_hidden_when_question_card_handles_it() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            tool_id: Some("toolu-question".to_string()),
            name: "ask_user_question".to_string(),
            input: serde_json::json!({
                "question": "Pick one",
                "options": [{"label": "A"}, {"label": "B"}]
            })
            .to_string(),
        })],
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(!output.contains("Activity"), "{output}");
    assert!(!output.contains("ask_user_question called"), "{output}");
}

#[test]
fn control_permission_tool_request_is_hidden_when_approval_card_handles_it() {
    let mut state = InlineState {
        language: Language::EnUs,
        approval_mode: CoshApprovalMode::Auto,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolPermissionRequest {
            run_id: "run-1".to_string(),
            request_id: "ctrl-write".to_string(),
            tool_name: "Write".to_string(),
            tool_input: serde_json::json!({
                "file_path": "/tmp/cosh-write.txt",
                "content": "ok"
            }),
            tool_use_id: "toolu-write".to_string(),
            hook_requires_approval: false,
        })],
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(!output.contains("Activity"), "{output}");
    assert!(!output.contains("Write requested"), "{output}");
}

#[test]
fn matching_tool_call_is_hidden_when_control_permission_card_handles_it() {
    let mut state = InlineState {
        language: Language::EnUs,
        approval_mode: CoshApprovalMode::Auto,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-write".to_string()),
                name: "Write".to_string(),
                input: serde_json::json!({
                    "file_path": "/tmp/cosh-write.txt",
                    "content": "ok"
                })
                .to_string(),
            }),
            governed(AgentEvent::ToolPermissionRequest {
                run_id: "run-1".to_string(),
                request_id: "ctrl-write".to_string(),
                tool_name: "Write".to_string(),
                tool_input: serde_json::json!({
                    "file_path": "/tmp/cosh-write.txt",
                    "content": "ok"
                }),
                tool_use_id: "toolu-write".to_string(),
                hook_requires_approval: false,
            }),
        ],
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(!output.contains("Activity"), "{output}");
    assert!(!output.contains("Write called"), "{output}");
    assert!(!output.contains("Write requested"), "{output}");
}

#[test]
fn recommend_mode_keeps_only_control_permission_row_for_matching_tool_call() {
    let mut state = InlineState {
        language: Language::EnUs,
        approval_mode: CoshApprovalMode::Recommend,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-read".to_string()),
                name: "Read".to_string(),
                input: serde_json::json!({ "file_path": "Cargo.toml" }).to_string(),
            }),
            governed(AgentEvent::ToolPermissionRequest {
                run_id: "run-1".to_string(),
                request_id: "ctrl-read".to_string(),
                tool_name: "Read".to_string(),
                tool_input: serde_json::json!({ "file_path": "Cargo.toml" }),
                tool_use_id: "toolu-read".to_string(),
                hook_requires_approval: false,
            }),
        ],
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(!output.contains("Read called"), "{output}");
    assert!(
        output.contains("Read requested: Cargo.toml; [Details]"),
        "{output}"
    );
}

#[test]
fn recommend_mode_keeps_control_permission_tool_request_activity() {
    let mut state = InlineState {
        language: Language::EnUs,
        approval_mode: CoshApprovalMode::Recommend,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[governed(AgentEvent::ToolPermissionRequest {
            run_id: "run-1".to_string(),
            request_id: "ctrl-write".to_string(),
            tool_name: "Write".to_string(),
            tool_input: serde_json::json!({
                "file_path": "/tmp/cosh-write.txt",
                "content": "ok"
            }),
            tool_use_id: "toolu-write".to_string(),
            hook_requires_approval: false,
        })],
    );

    let mut output = Vec::new();
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(
        output.contains("Write requested: /tmp/cosh-write.txt (new file); [Details]"),
        "{output}"
    );
}

#[test]
fn control_protocol_policy_suppresses_known_foreground_shell_echo() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    state
        .control
        .mark_provider_shell_transcript_seen("toolu-shell");
    let ids = record_activity_rows_with_policy(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-shell".to_string()),
                name: "run_shell_command".to_string(),
                input: r#"{"command":"df -h"}"#.to_string(),
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "toolu-shell".to_string(),
                stream: "stdout".to_string(),
                text: "Filesystem\n/dev/disk1\n".to_string(),
            }),
            governed(AgentEvent::ToolCompleted {
                run_id: "run-1".to_string(),
                tool_id: "toolu-shell".to_string(),
                status: "success".to_string(),
            }),
        ],
        ActivityRecordPolicy {
            suppress_provider_native_shell: true,
        },
    );

    assert!(ids.is_empty(), "{ids:?}");
    assert!(state.activity.rows.is_empty(), "{:?}", state.activity.rows);
}

#[test]
fn control_permission_shell_output_is_not_rendered_as_provider_native_transcript() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolPermissionRequest {
                run_id: "run-1".to_string(),
                request_id: "ctrl-1".to_string(),
                tool_name: "run_shell_command".to_string(),
                tool_input: serde_json::json!({ "command": "ssh -V" }),
                tool_use_id: "toolu-shell".to_string(),
                hook_requires_approval: false,
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "toolu-shell".to_string(),
                stream: "stdout".to_string(),
                text: "PROVIDER OUTPUT SHOULD NOT RENDER\n".to_string(),
            }),
        ],
    );

    let mut output = Vec::new();
    render_provider_native_shell_transcript(&mut state, &ids, &mut output)
        .expect("render shell transcript");
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(!output.contains("$ ssh -V"), "{output}");
    assert!(
        !output.contains("PROVIDER OUTPUT SHOULD NOT RENDER"),
        "{output}"
    );
}

#[test]
fn provider_native_streamed_shell_output_uses_tool_id_not_pending_order() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("tool-first".to_string()),
                name: "run_shell_command".to_string(),
                input: r#"{"command":"echo FIRST"}"#.to_string(),
            }),
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("tool-second".to_string()),
                name: "run_shell_command".to_string(),
                input: r#"{"command":"echo SECOND"}"#.to_string(),
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "tool-second".to_string(),
                stream: "stdout".to_string(),
                text: "SECOND\n".to_string(),
            }),
        ],
    );

    let mut output = Vec::new();
    render_provider_native_shell_transcript(&mut state, &ids, &mut output)
        .expect("render shell transcript");
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(output.contains("$ echo SECOND\nSECOND\n"), "{output}");
    assert!(!output.contains("$ echo FIRST\nSECOND"), "{output}");
    assert!(!output.contains("Activity"), "{output}");
}

#[test]
fn provider_native_shell_error_completion_uses_transcript_not_activity() {
    let mut state = InlineState {
        language: Language::EnUs,
        ..InlineState::default()
    };
    let ids = record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-1".to_string()),
                name: "run_shell_command".to_string(),
                input: serde_json::json!({ "command": "df -h" }).to_string(),
            }),
            governed(AgentEvent::ToolCompleted {
                run_id: "run-1".to_string(),
                tool_id: "toolu-1".to_string(),
                status: "error".to_string(),
            }),
        ],
    );

    let mut output = Vec::new();
    render_provider_native_shell_transcript(&mut state, &ids, &mut output)
        .expect("render shell transcript");
    render_activity_rows(&state, &ids, &mut output).expect("render activity");
    let output = String::from_utf8(output).expect("utf8 output");

    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("tool status: error"), "{output}");
    assert!(!output.contains("Activity"), "{output}");
    assert!(!output.contains("Tool error"), "{output}");
}

#[test]
fn tool_output_ref_uses_private_permissions() {
    let dir = std::env::temp_dir().join(format!(
        "cosh-shell-activity-output-ref-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);

    let path = write_tool_output_ref(&dir, "out-1", "secret-ish\n").expect("write output ref");

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
fn shell_handoff_activity_marks_user_interrupt_status() {
    let mut state = InlineState::default();
    let request = ShellHandoffRequest::new(
        "sleep 100",
        "$ sleep 100",
        "approved_provider_shell_tool",
        "user",
        "req-1",
        "run-1",
        0,
    )
    .expect("handoff request");
    state
        .control
        .shell_handoff_mut()
        .enqueue_approved_request(request);
    state
        .control
        .shell_handoff_mut()
        .emit_next_approved()
        .expect("emit pending handoff");
    let block = CommandBlock {
        id: "cmd-1".to_string(),
        session_id: "session-1".to_string(),
        command: "sleep 100".to_string(),
        origin: CommandOrigin::ProviderTool,
        cwd: "/tmp".to_string(),
        end_cwd: "/tmp".to_string(),
        started_at_ms: 1,
        ended_at_ms: 10,
        duration_ms: 9,
        exit_code: 130,
        status: CommandStatus::Failed,
        output: OutputRefs {
            terminal_output_ref: Some("/tmp/internal-output-ref.txt".to_string()),
            terminal_output_bytes: 0,
        },
    };

    let ids = record_approved_shell_handoff_blocks(&mut state, &[block]);

    assert_eq!(ids, vec!["handoff-1"]);
    let row = state
        .activity
        .rows
        .iter()
        .find(|row| row.id == "handoff-1")
        .expect("handoff row");
    assert_eq!(row.status, "interrupted");
    assert!(row.detail.contains("status: interrupted"), "{}", row.detail);
    assert!(row.detail.contains("exit_code: 130"), "{}", row.detail);
    assert!(
        row.detail
            .contains("output_id: terminal-output://session-1/cmd-1"),
        "{}",
        row.detail
    );
}

#[test]
fn shell_handoff_activity_ignores_stale_same_command_block_before_request() {
    let mut state = InlineState::default();
    let request = ShellHandoffRequest::new(
        "df -h",
        "$ df -h",
        "approved_provider_shell_tool",
        "user",
        "req-stale",
        "run-stale",
        1_000,
    )
    .expect("handoff request");
    state
        .control
        .shell_handoff_mut()
        .enqueue_approved_request(request);
    state
        .control
        .shell_handoff_mut()
        .emit_next_approved()
        .expect("emit pending handoff");
    let stale_block = CommandBlock {
        id: "cmd-stale".to_string(),
        session_id: "session-1".to_string(),
        command: "df -h".to_string(),
        origin: Default::default(),
        cwd: "/tmp".to_string(),
        end_cwd: "/tmp".to_string(),
        started_at_ms: 100,
        ended_at_ms: 200,
        duration_ms: 100,
        exit_code: 0,
        status: CommandStatus::Completed,
        output: OutputRefs {
            terminal_output_ref: Some("/tmp/stale-output-ref.txt".to_string()),
            terminal_output_bytes: 0,
        },
    };

    let ids = record_approved_shell_handoff_blocks(&mut state, &[stale_block]);

    assert!(ids.is_empty(), "{ids:?}");
    assert!(state.activity.rows.is_empty(), "{:?}", state.activity.rows);
    assert!(state.control.shell_handoff().pending_front().is_some());
}

#[test]
fn activity_interactive_handoff_summary_uses_state_language() {
    let mut state = InlineState {
        language: Language::ZhCn,
        ..InlineState::default()
    };
    record_activity_rows(
        &mut state,
        &[
            governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("tool-use-1".to_string()),
                name: "Bash".to_string(),
                input: serde_json::json!({ "command": "sudo systemctl status sshd" }).to_string(),
            }),
            governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "tool-use-1".to_string(),
                stream: "stderr".to_string(),
                text: "sudo: a terminal is required\n".to_string(),
            }),
            governed(AgentEvent::ToolCompleted {
                run_id: "run-1".to_string(),
                tool_id: "tool-use-1".to_string(),
                status: "error".to_string(),
            }),
        ],
    );

    let row = state
        .activity
        .rows
        .iter()
        .find(|row| row.id == "tool-2")
        .expect("activity row");
    assert_eq!(
        row.summary,
        "sudo: a terminal is required; 可能需要前台 shell；[Send to shell] handoff-1；[Details] tool-2"
    );
    assert!(row
        .detail
        .contains("interactive_hint: may_require_foreground_shell"));
}
