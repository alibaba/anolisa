use super::*;

#[test]
fn raw_cli_streamed_tool_fallback_with_host_capability_is_not_delivered() {
    let home = temp_shell_home("qwen-streamed-tool-fallback-no-delivery");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback","message":{"content":[{"type":"text","text":"STREAMED FALLBACK RECOVERY ONLY"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *streamed-tool-fallback-no-delivery*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_fallback","name":"run_shell_command","input":{"command":"echo STREAMED_FALLBACK"}}]}}'
    sleep 30
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"streamed-tool-fallback-no-delivery\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(5_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ echo STREAMED_FALLBACK"), "{output}");
    assert!(output.contains("STREAMED_FALLBACK"), "{output}");
    assert!(
        output.contains("STREAMED FALLBACK RECOVERY ONLY"),
        "{output}"
    );
    assert!(
        output.contains("selected_shell_execution_path: foreground_shell_handoff_recovery"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: not_provider_tool_request"),
        "{output}"
    );
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(
        !output.contains("control_protocol_host_executed_shell_result"),
        "{output}"
    );
}

#[test]
fn raw_cli_streamed_tool_fallback_recovery_blocks_new_shell_tool() {
    let home = temp_shell_home("qwen-streamed-tool-fallback-recovery-blocks-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback-block","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback-block","message":{"id":"m2","type":"message","role":"assistant","model":"qwen","content":[{"type":"text","text":"RECOVERY TRIED SECOND SHELL TOOL"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback-block","message":{"id":"m3","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_recovery","name":"run_shell_command","input":{"command":"echo SHOULD_NOT_RUN_IN_RECOVERY"}}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback-block","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback-block","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *streamed-tool-fallback-blocks-recovery-tool*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback-block","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_fallback","name":"run_shell_command","input":{"command":"echo STREAMED_ONCE"}}]}}'
    sleep 30
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback-block","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[
            ("HOME", &home_str),
            ("PATH", &path),
            ("COSH_SHELL_EVIDENCE_IDLE_TIMEOUT_SECS", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"streamed-tool-fallback-blocks-recovery-tool\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(5_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ echo STREAMED_ONCE"), "{output}");
    assert!(output.contains("STREAMED_ONCE"), "{output}");
    assert!(
        output.contains("RECOVERY TRIED SECOND SHELL TOOL"),
        "{output}"
    );
    assert_eq!(
        count_occurrences(&output, "Bash tool sent to shell"),
        1,
        "{output}"
    );
    assert!(
        !output.contains("cosh-osc$ echo SHOULD_NOT_RUN_IN_RECOVERY"),
        "{output}"
    );
    assert!(!output.contains("Auto-approved req-2"), "{output}");
    assert!(
        output.contains("provider_result_delivery_status: not_provider_tool_request"),
        "{output}"
    );
}

#[test]
fn raw_cli_qwen_streamed_non_shell_tool_renders_activity_card() {
    let home = temp_shell_home("qwen-streamed-non-shell-tool-activity");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-tool-activity","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *streamed-non-shell-tool-activity*)
    printf '%s\n' '{"type":"stream_event","session_id":"sess-tool-activity","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_read","name":"Read","input":{}}}}'
    printf '%s\n' '{"type":"stream_event","session_id":"sess-tool-activity","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"Cargo.toml\"}"}}}'
    printf '%s\n' '{"type":"stream_event","session_id":"sess-tool-activity","event":{"type":"content_block_stop","index":0}}'
    printf '%s\n' '{"type":"user","session_id":"sess-tool-activity","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_read","is_error":false,"content":"read output"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-tool-activity","message":{"role":"assistant","content":[{"type":"text","text":"READ VISIBILITY FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-tool-activity","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-tool-activity","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? streamed-non-shell-tool-activity\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(1_500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Read called: Cargo.toml; [Details] tool-1"),
        "{output}"
    );
    assert!(output.contains("Activity details tool-1"), "{output}");
    assert!(output.contains("evidence: ProviderToolCall"), "{output}");
    assert!(
        output.contains("provider: provider_native_stream"),
        "{output}"
    );
    assert!(output.contains("tool_name: Read"), "{output}");
    assert!(output.contains("input_preview: Cargo.toml"), "{output}");
    assert!(output.contains("READ VISIBILITY FINAL"), "{output}");
}

#[test]
fn raw_cli_non_shell_permission_passes_through_allow_only() {
    let home = temp_shell_home("qwen-non-shell-pass-through");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-non-shell-pass-through","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-read-pass-through*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-read","request":{"subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"README.md"},"tool_use_id":"toolu-read"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ctrl-read"'*'"behavior":"allow"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-non-shell-pass-through","message":{"content":[{"type":"text","text":"Read permission allowed through provider control protocol."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-non-shell-pass-through","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-non-shell-pass-through","is_error":true,"result":"missing non-shell allow response"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-non-shell-pass-through","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-read-pass-through\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(3_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(
        output.contains("Read permission allowed through provider control protocol."),
        "{output}"
    );
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
    assert!(
        !output.contains("missing non-shell allow response"),
        "{output}"
    );
}
