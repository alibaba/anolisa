use super::*;

#[test]
fn raw_cli_host_executed_shell_result_continues_same_provider_turn() {
    let home = temp_shell_home("qwen-host-executed-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let ssh_path = bin_dir.join("ssh");
    write_executable(
        &ssh_path,
        "#!/bin/sh\nprintf '%s\\n' 'OpenSSH_test foreground handoff'\n",
    );
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'df -h'*)
          printf '%s\n' '{"type":"user","session_id":"sess-host-executed","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu-1","is_error":false,"content":"PROVIDER_ECHO_SHOULD_NOT_RENDER_AS_ACTIVITY\n"}]}}'
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed","message":{"content":[{"type":"text","text":"Host-executed shell result received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed","is_error":true,"result":"missing host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed","is_error":false,"result":"ignored"}'
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
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider-host-executed-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        !output.contains("Tool output: stdout captured; [Details]"),
        "{output}"
    );
    assert!(
        !output.contains("Tool 输出: stdout 已捕获；[Details]"),
        "{output}"
    );
    assert!(
        !output.contains("PROVIDER_ECHO_SHOULD_NOT_RENDER_AS_ACTIVITY"),
        "{output}"
    );
    assert!(
        output.contains("Host-executed shell result received in same provider turn."),
        "{output}"
    );
    assert!(
        !output.contains("missing host_executed_shell result"),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains(
            "path_selection_reason: provider advertised host-executed shell result support"
        ),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(!output.contains("output_ref:"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_host_executed_streaming_order_renders_shell_before_post_text() {
    let home = temp_shell_home("qwen-host-executed-streaming-order");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-stream-order","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *host-executed-stream-order*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-stream-order","message":{"content":[{"type":"text","text":"HOST EXECUTED PRE TEXT STREAMS"}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'df -h'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-stream-order","message":{"content":[{"type":"text","text":"HOST EXECUTED POST TEXT WAITS"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-stream-order","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-stream-order","is_error":true,"result":"missing host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-stream-order","is_error":false,"result":"ignored"}'
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
                b"?? host-executed-stream-order\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(
        !output.contains("missing host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent 恢复"), "{output}");
    assert_ordered(
        &normalized,
        &[
            "HOST EXECUTED PRE TEXT STREAMS",
            "$ df -h",
            "Filesystem",
            "HOST EXECUTED POST TEXT WAITS",
        ],
    );
}

#[test]
fn raw_cli_manual_approval_host_executed_shell_result_continues_same_turn() {
    let home = temp_shell_home("qwen-manual-host-executed-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-manual-host-executed","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *manual-provider-host-executed-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-manual","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"touch \"$HOME/manual-host-executed-ok\""},"tool_use_id":"toolu-manual"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'manual-host-executed-ok'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-manual-host-executed","message":{"content":[{"type":"text","text":"Manual host-executed shell result received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-manual-host-executed","is_error":false,"result":"done"}'
          exit 0
          ;;
        *'"behavior":"allow"'*)
          printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-manual-host-executed","is_error":true,"result":"unexpected provider-native allow"}'
          exit 1
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-manual-host-executed","is_error":true,"result":"missing manual host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-manual-host-executed","is_error":false,"result":"ignored"}'
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
                b"?? manual-provider-host-executed-shell\n".to_vec(),
                Duration::from_millis(300),
            ),
            (b"\n".to_vec(), Duration::from_millis(4_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(8_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("$ touch \"$HOME/manual-host-executed-ok\""),
        "{output}"
    );
    assert!(
        output.contains("Manual host-executed shell result received in same provider turn."),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
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
fn raw_cli_host_executed_nonzero_exit_returns_normal_tool_result() {
    let home = temp_shell_home("qwen-host-executed-nonzero");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-nonzero","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-nonzero*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-nonzero","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"false"},"tool_use_id":"toolu-nonzero"}}'
    if IFS= read -r response; then
      case "$response" in
        *host_executed_shell*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-nonzero","message":{"content":[{"type":"text","text":"Host-executed nonzero result received as normal tool result."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-nonzero","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-nonzero","is_error":true,"result":"missing nonzero host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-nonzero","is_error":false,"result":"ignored"}'
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
                b"?? provider-host-executed-nonzero\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"true\nexit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ false"), "{output}");
    assert!(output.contains("Shell: failed · req-1"), "{output}");
    assert!(
        output.contains("Host-executed nonzero result received as normal tool result."),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("status: failed"), "{output}");
    assert!(output.contains("exit_code: 1"), "{output}");
    assert!(
        !output.contains("missing nonzero host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("The command false failed"), "{output}");
}

#[test]
fn raw_cli_host_executed_interrupt_returns_normal_tool_result() {
    let home = temp_shell_home("qwen-host-executed-interrupt");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-interrupt","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-interrupt*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-interrupt","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"sh -c '\''exit 130'\''"},"tool_use_id":"toolu-interrupt"}}'
    if IFS= read -r response; then
      case "$response" in
        *host_executed_shell*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-interrupt","message":{"content":[{"type":"text","text":"Host-executed interrupt result received as normal tool result."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-interrupt","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-interrupt","is_error":true,"result":"missing interrupt host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-interrupt","is_error":false,"result":"ignored"}'
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
                b"?? provider-host-executed-interrupt\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"true\nexit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ sh -c 'exit 130'"), "{output}");
    assert!(output.contains("Shell: interrupted · req-1"), "{output}");
    assert!(
        output.contains("Host-executed interrupt result received as normal tool result."),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("status: interrupted"), "{output}");
    assert!(output.contains("exit_code: 130"), "{output}");
    assert!(
        !output.contains("missing interrupt host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(
        !output.contains("The command sh -c 'exit 130' failed"),
        "{output}"
    );
}

#[test]
fn raw_cli_host_executed_multi_tool_keeps_single_turn_boundary() {
    let home = temp_shell_home("qwen-host-executed-multi-tool");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-multi","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-multi","message":{"content":[{"type":"text","text":"UNEXPECTED FRESH CONTINUATION"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-multi","is_error":false,"result":"unexpected"}'
    exit 0
    ;;
esac
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-multi","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-multi-tool*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
    IFS= read -r response1 || exit 2
    case "$response1" in
      *'"behavior":"host_executed_shell"'*'df -h'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-multi","is_error":true,"result":"missing first host result"}'; exit 1 ;;
    esac
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-multi","message":{"content":[{"type":"text","text":"FIRST TOOL ANALYSIS TEXT"}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-2","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"du -sh ."},"tool_use_id":"toolu-2"}}'
    IFS= read -r response2 || exit 2
    case "$response2" in
      *'"behavior":"host_executed_shell"'*'du -sh .'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-multi","is_error":true,"result":"missing second host result"}'; exit 1 ;;
    esac
    sleep 2
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-multi","message":{"content":[{"type":"text","text":"FINAL MULTI TOOL REPORT"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-multi","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-multi","is_error":false,"result":"ignored"}'
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
                b"?? provider-host-executed-multi-tool\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"echo AFTER_PROVIDER_INPUT\n".to_vec(),
                Duration::from_millis(1_500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(3_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Auto-approved req-2"), "{output}");
    assert!(output.contains("FIRST TOOL ANALYSIS TEXT"), "{output}");
    assert!(output.contains("FINAL MULTI TOOL REPORT"), "{output}");
    assert!(!output.contains("missing first host result"), "{output}");
    assert!(!output.contains("missing second host result"), "{output}");
    assert!(
        !output.contains("UNEXPECTED FRESH CONTINUATION"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("Agent 恢复"), "{output}");
    assert!(!output.contains("Using a fresh provider turn"), "{output}");
    assert!(!output.contains("Shell recovery"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert_eq!(
        count_occurrences_between(&normalized, "\t.\n", "FINAL MULTI TOOL REPORT", "cosh-osc$"),
        0,
        "{output}"
    );
    assert_eq!(
        count_occurrences_between(
            &normalized,
            "\t.\n",
            "FINAL MULTI TOOL REPORT",
            "Thinking..."
        ),
        0,
        "{output}"
    );
    assert!(
        !normalized.contains("cosh-osc$ cosh-osc$ echo AFTER_PROVIDER_INPUT"),
        "{output}"
    );
    assert_inline_before_followup(
        &normalized,
        "FINAL MULTI TOOL REPORT",
        "AFTER_PROVIDER_INPUT",
    );
}

#[test]
fn raw_cli_host_executed_provider_disconnect_marks_recovery_reason() {
    let home = temp_shell_home("qwen-host-executed-disconnect");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-disconnect","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-disconnect*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"sleep 1; df -h"},"tool_use_id":"toolu-1"}}'
    kill -9 "$$"
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-disconnect","is_error":false,"result":"ignored"}'
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
            (b"/mode approval trust confirm\n".to_vec(), Duration::ZERO),
            (
                b"?? provider-host-executed-disconnect\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ sleep 1; df -h"), "{output}");
    assert!(
        output.contains("selected_shell_execution_path: foreground_shell_handoff_recovery"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: provider_run_not_active")
            || output.contains("provider_result_delivery_status: provider_channel_closed"),
        "{output}"
    );
    assert!(
        output.contains("recovery_reason: provider run was not active")
            || output.contains("recovery_reason: provider approval channel closed"),
        "{output}"
    );
    assert!(
        output.contains("latest recovery status: provider_run_not_active")
            || output.contains("latest recovery status: provider_channel_closed"),
        "{output}"
    );
    assert!(
        output.contains("latest recovery reason: provider run was not active")
            || output.contains("latest recovery reason: provider approval channel closed"),
        "{output}"
    );
    assert!(
        !output.contains("control_protocol_host_executed_shell_result"),
        "{output}"
    );
}
