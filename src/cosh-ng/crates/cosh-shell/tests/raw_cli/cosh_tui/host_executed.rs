use super::*;

#[test]
fn raw_cli_cosh_tui_host_executed_shell_result_continues_same_provider_turn() {
    let home = temp_shell_home("cosh-tui-host-executed-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-1","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-1"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'df -h'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed","message":{"content":[{"type":"text","text":"Cosh-tui host-executed shell result received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed","is_error":true,"result":"missing cosh-tui host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-shell\n".to_vec(),
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
        output.contains("Cosh-tui host-executed shell result received in same provider turn."),
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
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(
        output.contains("host-executed shell result: delivered"),
        "{output}"
    );
    assert!(
        output
            .contains("selected shell execution path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains("latest provider request: ctrl-cosh-tui-1"),
        "{output}"
    );
    assert!(
        output.contains("latest tool use id: toolu-cosh-tui-1"),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(
        !output.contains("missing cosh-tui host_executed_shell result"),
        "{output}"
    );
    assert!(
        !output.contains("bash: cosh-tui-provider-host-executed-shell: command not found"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_auto_safe_shell_auto_approves_host_executed() {
    let home = temp_shell_home("cosh-tui-auto-safe-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-auto-safe","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-auto-safe-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-auto-safe","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-auto-safe"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'df -h'*'Filesystem'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-auto-safe","message":{"content":[{"type":"text","text":"AUTO SAFE HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-auto-safe","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-auto-safe","is_error":true,"result":"missing auto host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-auto-safe","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-auto-safe-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(output.contains("AUTO SAFE HOSTEXEC RECEIVED"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(
        !output.contains("missing auto host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_tui_sysctl_non_ascii_shell_handoff_is_not_intercepted() {
    let home = temp_shell_home("cosh-tui-sysctl-non-ascii-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-sysctl-non-ascii","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-sysctl-non-ascii-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-sysctl-non-ascii","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"sysctl -n kernel.ostype 2>/dev/null || printf sysctl-fallback; printf '\'' 内存总计\\n'\''"},"tool_use_id":"toolu-sysctl-non-ascii"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'sysctl-fallback'*'内存总计'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-sysctl-non-ascii","message":{"content":[{"type":"text","text":"SYSCTL NON ASCII HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-sysctl-non-ascii","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-sysctl-non-ascii","is_error":true,"result":"missing sysctl non-ascii host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-sysctl-non-ascii","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval trust confirm\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-sysctl-non-ascii-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("内存总计"), "{output}");
    assert!(
        output.contains("SYSCTL NON ASCII HOSTEXEC RECEIVED"),
        "{output}"
    );
    assert!(!output.contains("natural_language"), "{output}");
    assert!(
        !output.contains("missing sysctl non-ascii host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_tui_trust_confirm_shell_auto_approves_host_executed() {
    let home = temp_shell_home("cosh-tui-trust-confirm-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-trust-confirm","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-trust-confirm-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-trust-confirm","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"printf trust-confirm-hostexec"},"tool_use_id":"toolu-trust-confirm"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'printf trust-confirm-hostexec'*'trust-confirm-hostexec'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-trust-confirm","message":{"content":[{"type":"text","text":"TRUST CONFIRM HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-trust-confirm","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-trust-confirm","is_error":true,"result":"missing trust host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-trust-confirm","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval trust confirm\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-trust-confirm-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(
        output.contains("$ printf trust-confirm-hostexec"),
        "{output}"
    );
    assert!(output.contains("trust-confirm-hostexec"), "{output}");
    assert!(
        output.contains("TRUST CONFIRM HOSTEXEC RECEIVED"),
        "{output}"
    );
    assert!(!output.contains("Approval required"), "{output}");
    assert!(
        !output.contains("missing trust host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_tui_duplicate_host_executed_shell_request_is_not_executed_twice() {
    let home = temp_shell_home("cosh-tui-host-executed-duplicate");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-duplicate","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-duplicate*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-dup","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-dup"}}'
    IFS= read -r response1 || exit 2
    case "$response1" in
      *'"behavior":"host_executed_shell"'*'df -h'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":true,"result":"missing first host result"}'; exit 1 ;;
    esac
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-dup","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-dup"}}'
    IFS= read -r response2 || exit 2
    case "$response2" in
      *'"behavior":"deny"'*'Duplicate shell tool request was already completed'*)
        printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-duplicate","message":{"content":[{"type":"text","text":"Duplicate host-executed shell request denied without second execution."}]}}'
        printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":false,"result":"done"}'
        exit 0
        ;;
    esac
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":true,"result":"duplicate request was not denied"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-duplicate\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(7_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("Duplicate host-executed shell request denied without second execution."),
        "{output}"
    );
    assert!(
        !output.contains("duplicate request was not denied"),
        "{output}"
    );
    assert_eq!(count_occurrences(&normalized, "\ndf -h\n"), 1, "{output}");
    assert_eq!(count_occurrences(&normalized, "Filesystem"), 1, "{output}");
    assert!(
        output.contains("host-executed shell result: delivered"),
        "{output}"
    );
    assert!(
        output.contains("latest provider request: ctrl-cosh-tui-dup"),
        "{output}"
    );
    assert!(
        output.contains("latest tool use id: toolu-cosh-tui-dup"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_nonzero_returns_normal_tool_result() {
    let home = temp_shell_home("cosh-tui-host-executed-nonzero");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-nonzero","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-nonzero*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-nonzero","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"false"},"tool_use_id":"toolu-cosh-tui-nonzero"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'"exit_code":1'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-nonzero","message":{"content":[{"type":"text","text":"Cosh-tui nonzero host-executed result received as normal tool result."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-nonzero","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-nonzero","is_error":true,"result":"missing nonzero host result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-nonzero","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-nonzero\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"true\n".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ false"), "{output}");
    assert!(output.contains("Shell: failed · req-1"), "{output}");
    assert!(
        output.contains("Cosh-tui nonzero host-executed result received as normal tool result."),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("status: failed"), "{output}");
    assert!(output.contains("exit_code: 1"), "{output}");
    assert!(!output.contains("missing nonzero host result"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("The command false failed"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_long_command_continues_same_turn() {
    let home = temp_shell_home("cosh-tui-host-executed-long");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-long","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-long*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-long","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"sleep 1; echo hostexec-done"},"tool_use_id":"toolu-cosh-tui-long"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'sleep 1; echo hostexec-done'*'hostexec-done'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-long","message":{"content":[{"type":"text","text":"Cosh-tui long host-executed command continued in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-long","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-long","is_error":true,"result":"missing long host result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-long","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-long\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(7_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ sleep 1; echo hostexec-done"), "{output}");
    assert!(output.contains("hostexec-done"), "{output}");
    assert!(
        output.contains("Cosh-tui long host-executed command continued in same provider turn."),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(!output.contains("missing long host result"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_large_output_is_bounded() {
    let home = temp_shell_home("cosh-tui-host-executed-large");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-large","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-large*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-large","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"printf %08000d 0"},"tool_use_id":"toolu-cosh-tui-large"}}'
    if IFS= read -r response; then
      response_len=${#response}
      case "$response" in
        *'"behavior":"host_executed_shell"'*'bounded_output_summary'*)
          if [ "$response_len" -gt 7000 ]; then
            printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-large","is_error":true,"result":"host result was not bounded"}'
            exit 1
          fi
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-large","message":{"content":[{"type":"text","text":"Cosh-tui large host-executed output was bounded for provider."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-large","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-large","is_error":true,"result":"missing bounded large host result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-large","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-large\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(7_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ printf %08000d 0"), "{output}");
    assert!(
        output.contains("Cosh-tui large host-executed output was bounded for provider."),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(
        !output.contains("missing bounded large host result"),
        "{output}"
    );
    assert!(!output.contains("host result was not bounded"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_multi_tool_keeps_single_turn_boundary() {
    let home = temp_shell_home("cosh-tui-host-executed-multi-tool");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-multi","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-multi-tool*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-multi-1","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-multi-1"}}'
    IFS= read -r response1 || exit 2
    case "$response1" in
      *'"behavior":"host_executed_shell"'*'df -h'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-multi","is_error":true,"result":"missing first cosh-tui host result"}'; exit 1 ;;
    esac
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-multi","message":{"content":[{"type":"text","text":"FIRST COSH-TUI TOOL ANALYSIS TEXT"}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-multi-2","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"du -sh ."},"tool_use_id":"toolu-cosh-tui-multi-2"}}'
    IFS= read -r response2 || exit 2
    case "$response2" in
      *'"behavior":"host_executed_shell"'*'du -sh .'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-multi","is_error":true,"result":"missing second cosh-tui host result"}'; exit 1 ;;
    esac
    sleep 2
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-multi","message":{"content":[{"type":"text","text":"FINAL COSH-TUI MULTI TOOL REPORT"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-multi","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-multi","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-multi-tool\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"echo AFTER_COSH_TUI_PROVIDER_INPUT\n".to_vec(),
                Duration::from_millis(1_500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(3_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Auto-approved req-2"), "{output}");
    assert!(
        output.contains("FIRST COSH-TUI TOOL ANALYSIS TEXT"),
        "{output}"
    );
    assert!(
        output.contains("FINAL COSH-TUI MULTI TOOL REPORT"),
        "{output}"
    );
    assert!(
        !output.contains("missing first cosh-tui host result"),
        "{output}"
    );
    assert!(
        !output.contains("missing second cosh-tui host result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("Agent 恢复"), "{output}");
    assert!(!output.contains("Using a fresh provider turn"), "{output}");
    assert!(!output.contains("Shell recovery"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert_eq!(
        count_occurrences_between(
            &normalized,
            "\t.\n",
            "FINAL COSH-TUI MULTI TOOL REPORT",
            "cosh-osc$"
        ),
        0,
        "{output}"
    );
    assert_inline_before_followup(
        &normalized,
        "FINAL COSH-TUI MULTI TOOL REPORT",
        "AFTER_COSH_TUI_PROVIDER_INPUT",
    );
}
