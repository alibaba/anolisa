use super::*;

#[test]
fn raw_cli_cosh_tui_suppresses_provider_native_echo_after_manual_host_executed_shell() {
    let home = temp_shell_home("cosh-tui-manual-host-executed-echo");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-manual-host-executed-echo","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-manual-host-executed-echo*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-manual-echo","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"sudo -V"},"tool_use_id":"toolu-cosh-tui-manual-echo"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'sudo -V'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-manual-host-executed-echo","message":{"content":[{"type":"text","text":"Manual host-executed result accepted."},{"type":"tool_use","id":"toolu-cosh-tui-manual-echo-provider","name":"shell","input":{"command":"sudo -V"}}]}}'
          printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-manual-host-executed-echo","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu-cosh-tui-manual-echo-provider","is_error":false,"content":"PROVIDER ECHO SHOULD BE SUPPRESSED\n"}]}}'
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-manual-host-executed-echo","message":{"content":[{"type":"text","text":"COSH TUI HOST EXECUTED ECHO FINAL"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":false,"result":"done"}'
          exit 0
          ;;
        *'"behavior":"allow"'*)
          printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":true,"result":"unexpected provider-native allow"}'
          exit 1
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":true,"result":"missing manual host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (
                b"?? cosh-tui-manual-host-executed-echo\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (b"/debug session\n".to_vec(), Duration::from_millis(6_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-2"), "{output}");
    assert!(!output.contains("auto-approved by provider"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(normalized.contains("\nsudo -V\n"), "{output}");
    assert!(
        output.contains("COSH TUI HOST EXECUTED ECHO FINAL"),
        "{output}"
    );
    assert!(
        !output.contains("PROVIDER ECHO SHOULD BE SUPPRESSED"),
        "{output}"
    );
    assert!(
        !output.contains("unexpected provider-native allow"),
        "{output}"
    );
    assert!(
        !output.contains("missing manual host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_provider_native_tool_results_are_visible() {
    let home = temp_shell_home("cosh-tui-provider-native-tool-visible");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-native-visible","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-native-visible*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-visible","message":{"content":[{"type":"tool_use","id":"call_cosh_tui_shell","name":"shell","input":{"command":"echo COSH_TUI_NATIVE_SHELL"}}]}}'
    printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-native-visible","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_cosh_tui_shell","is_error":false,"content":"COSH_TUI_NATIVE_SHELL\n"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-visible","message":{"content":[{"type":"text","text":"COSH TUI PROVIDER NATIVE VISIBLE FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-visible","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-visible","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-provider-native-visible\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(1_500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("$ echo COSH_TUI_NATIVE_SHELL"), "{output}");
    assert!(output.contains("COSH_TUI_NATIVE_SHELL"), "{output}");
    assert!(
        output.contains("auto-approved by provider: $ echo COSH_TUI_NATIVE_SHELL; [Details]"),
        "{output}"
    );
    assert!(
        output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
    assert!(
        output.contains("COSH TUI PROVIDER NATIVE VISIBLE FINAL"),
        "{output}"
    );
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("missing host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_provider_native_shell_omits_duplicate_activity_in_normal_ui() {
    let home = temp_shell_home("cosh-tui-provider-native-shell-no-activity");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-native-no-activity","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-native-no-activity*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-no-activity","message":{"content":[{"type":"tool_use","id":"call_shell_no_activity","name":"shell","input":{"command":"echo COSH_TUI_NATIVE_NO_ACTIVITY"}}]}}'
    printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-native-no-activity","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_shell_no_activity","is_error":false,"content":"COSH_TUI_NATIVE_NO_ACTIVITY\n"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-no-activity","message":{"content":[{"type":"text","text":"COSH TUI PROVIDER NATIVE NO ACTIVITY FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-no-activity","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-no-activity","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-provider-native-no-activity\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("$ echo COSH_TUI_NATIVE_NO_ACTIVITY"),
        "{output}"
    );
    assert!(output.contains("COSH_TUI_NATIVE_NO_ACTIVITY"), "{output}");
    assert!(
        output.contains("COSH TUI PROVIDER NATIVE NO ACTIVITY FINAL"),
        "{output}"
    );
    assert!(!output.contains("auto-approved by provider"), "{output}");
    assert!(
        !output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_tui_control_request_suppresses_matching_shell_snapshot() {
    let home = temp_shell_home("cosh-tui-control-suppresses-shell-snapshot");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-control-snapshot","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-control-snapshot-dup*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-control-snapshot","message":{"content":[{"type":"tool_use","id":"toolu-cosh-tui-dup","name":"shell","input":{"command":"df -h"}}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-dup","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-dup"}}'
    IFS= read -r response || exit 2
    case "$response" in
      *'"behavior":"host_executed_shell"'*'df -h'*)
        printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-control-snapshot","message":{"content":[{"type":"text","text":"COSH TUI CONTROL SNAPSHOT FINAL"}]}}'
        printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-control-snapshot","is_error":false,"result":"done"}'
        exit 0
        ;;
    esac
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-control-snapshot","is_error":true,"result":"missing host executed result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-control-snapshot","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-control-snapshot-dup\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(2_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        output.contains("COSH TUI CONTROL SNAPSHOT FINAL"),
        "{output}"
    );
    assert!(!output.contains("auto-approved by provider"), "{output}");
    assert!(
        !output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
    assert!(!output.contains("missing host executed result"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_streamed_provider_native_shell_result_renders_before_final_text() {
    let home = temp_shell_home("cosh-tui-streamed-provider-native-shell-visible");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-streamed-native-visible","model":"cosh-tui-test"}'
read -r user_message
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"message_start"}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"COSH TUI PRE TOOL TEXT STREAMS"}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_cosh_tui_stream_shell","name":"shell","input":{}}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"echo COSH_TUI_STREAM_NATIVE_SHELL\"}"}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_stop","index":0}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"COSH TUI POST TOOL TEXT SHOULD WAIT"}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"message_stop"}}'
sleep 1
printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-streamed-native-visible","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_cosh_tui_stream_shell","is_error":false,"content":"COSH_TUI_STREAM_NATIVE_SHELL\n"}]}}'
printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-streamed-native-visible","message":{"content":[{"type":"text","text":"COSH TUI STREAM PROVIDER NATIVE FINAL"}]}}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-streamed-native-visible","is_error":false,"result":"done"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-streamed-provider-native-visible\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(3_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    let command_pos = output
        .find("$ echo COSH_TUI_STREAM_NATIVE_SHELL")
        .unwrap_or_else(|| panic!("{output}"));
    let stdout_pos = output
        .find("COSH_TUI_STREAM_NATIVE_SHELL")
        .unwrap_or_else(|| panic!("{output}"));
    let pre_tool_text_pos = output
        .find("COSH TUI PRE TOOL TEXT STREAMS")
        .unwrap_or_else(|| panic!("{output}"));
    let post_tool_text_pos = output
        .find("COSH TUI POST TOOL TEXT SHOULD WAIT")
        .unwrap_or_else(|| panic!("{output}"));
    let final_pos = output
        .find("COSH TUI STREAM PROVIDER NATIVE FINAL")
        .unwrap_or_else(|| panic!("{output}"));
    assert!(pre_tool_text_pos < command_pos, "{output}");
    assert!(command_pos < post_tool_text_pos, "{output}");
    assert!(command_pos < final_pos, "{output}");
    assert!(stdout_pos < post_tool_text_pos, "{output}");
    assert!(stdout_pos < final_pos, "{output}");
    assert!(
        output
            .contains("auto-approved by provider: $ echo COSH_TUI_STREAM_NATIVE_SHELL; [Details]"),
        "{output}"
    );
    assert!(
        output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_provider_native_non_shell_result_is_visible() {
    let home = temp_shell_home("cosh-tui-provider-native-non-shell-visible");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-native-non-shell-visible","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-native-non-shell-visible*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-non-shell-visible","message":{"content":[{"type":"tool_use","id":"call_cosh_tui_read","name":"Read","input":{"file_path":"Cargo.toml"}}]}}'
    printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-native-non-shell-visible","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_cosh_tui_read","is_error":false,"content":"read output visible"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-non-shell-visible","message":{"content":[{"type":"text","text":"COSH TUI PROVIDER NATIVE NON SHELL FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-non-shell-visible","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-non-shell-visible","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-provider-native-non-shell-visible\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(1_500)),
            (b"/details out-1\n".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Read called: Cargo.toml; [Details]"),
        "{output}"
    );
    assert!(output.contains("evidence: ProviderToolCall"), "{output}");
    assert!(output.contains("input_preview: Cargo.toml"), "{output}");
    assert!(output.contains("Tool output - stdout captured"), "{output}");
    assert!(output.contains("read output visible"), "{output}");
    assert!(
        output.contains("COSH TUI PROVIDER NATIVE NON SHELL FINAL"),
        "{output}"
    );
}
