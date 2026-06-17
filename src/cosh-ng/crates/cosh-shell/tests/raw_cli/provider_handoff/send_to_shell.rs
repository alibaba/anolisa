use super::*;

#[test]
fn raw_cli_provider_tool_interactive_escape_hatch_requires_explicit_send() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"?? provider interactive failure\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"/details out-1\n/details tool-2\n".to_vec(),
                Duration::from_millis(2_500),
            ),
            (
                b"/send-to-shell handoff-1\n".to_vec(),
                Duration::from_millis(1_000),
            ),
            (
                b"echo after-handoff\n".to_vec(),
                Duration::from_millis(2_000),
            ),
            (
                b"/details handoff-1\nexit\n".to_vec(),
                Duration::from_millis(1_000),
            ),
        ],
    );

    assert!(output.contains("may require foreground shell"), "{output}");
    assert!(output.contains("[Send to shell]"), "{output}");
    assert!(output.contains("handoff-1"), "{output}");
    assert!(
        output.contains("Tool error: sudo: a terminal is required"),
        "{output}"
    );
    assert!(output.contains("sudo: a terminal is required"), "{output}");
    assert!(output.contains("Sending to shell"), "{output}");
    assert!(output.contains("$ git status"), "{output}");
    assert!(output.contains("Activity details handoff-1"), "{output}");
    assert!(
        output.contains("evidence: ShellCommandCompleted"),
        "{output}"
    );
    assert!(
        output.contains("execution_path: foreground_shell_pty"),
        "{output}"
    );
    assert!(output.contains("preview_hash: fnv1a64:"), "{output}");
    assert!(output.contains("actor: user"), "{output}");
    assert!(output.contains("source: send_to_shell"), "{output}");
    assert!(output.contains("tool_use_id: toolu-tty"), "{output}");
    assert!(output.contains("redaction_status: ref_only"), "{output}");
    assert!(
        output.contains("start a new Agent turn after the shell command completes"),
        "{output}"
    );
    assert!(output.contains("after-handoff"), "{output}");
    assert!(!output.contains("Tool result for request"), "{output}");
    assert!(!output.contains("Governance:"), "{output}");
}

#[test]
fn raw_cli_provider_tool_send_to_shell_uses_zh_language_env() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (
                b"?? provider interactive failure\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"/details out-1\n/details tool-2\n".to_vec(),
                Duration::from_millis(2_500),
            ),
            (
                b"/send-to-shell handoff-1\n".to_vec(),
                Duration::from_millis(1_000),
            ),
            (
                b"echo after-zh-handoff\n".to_vec(),
                Duration::from_millis(2_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );

    assert!(output.contains("可能需要前台 shell"), "{output}");
    assert!(output.contains("正在发送到 shell"), "{output}");
    assert!(
        output.contains("handoff-1 将在前台 shell 中运行。"),
        "{output}"
    );
    assert!(output.contains("$ git status"), "{output}");
    assert!(output.contains("after-zh-handoff"), "{output}");
    assert!(!output.contains("Sending to shell"), "{output}");
    assert!(
        !output.contains("will run in the foreground shell"),
        "{output}"
    );
    assert!(!output.contains("may require foreground shell"), "{output}");
    assert!(!output.contains("bash: /send-to-shell"), "{output}");
}

#[test]
fn raw_cli_missing_send_to_shell_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/send-to-shell missing-handoff\n\
         echo after-missing-handoff\n\
         exit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("Shell handoff 未找到"), "{output}");
    assert!(
        output.contains("missing-handoff 不可用；请先对 provider tool failure 使用 Details 操作"),
        "{output}"
    );
    assert!(!output.contains("Shell handoff not found"), "{output}");
    assert!(output.contains("after-missing-handoff"), "{output}");
    assert!(!output.contains("bash: /send-to-shell"), "{output}");
}

#[test]
fn raw_cli_approved_shell_handoff_command_not_found_does_not_intercept() {
    let home = temp_shell_home("cosh-tui-handoff-command-not-found");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-handoff-command-not-found","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-handoff-command-not-found*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-handoff-command-not-found","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"/help"},"tool_use_id":"toolu-handoff-command-not-found"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'/help'*'"exit_code":127'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-handoff-command-not-found","message":{"content":[{"type":"text","text":"HANDOFF COMMAND NOT FOUND HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-handoff-command-not-found","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-handoff-command-not-found","is_error":true,"result":"missing command-not-found host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-handoff-command-not-found","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-handoff-command-not-found\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("HANDOFF COMMAND NOT FOUND HOSTEXEC RECEIVED"),
        "{output}"
    );
    assert!(!output.contains("intercepted  slash"), "{output}");
    assert!(
        !output.contains("missing command-not-found host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_approved_shell_handoff_bypasses_marker_intercepts() {
    let home = temp_shell_home("cosh-tui-handoff-bypass-marker");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-handoff-bypass-marker","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-handoff-bypass-marker*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-handoff-bypass-marker","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"?? should-run-as-shell"},"tool_use_id":"toolu-handoff-bypass-marker"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'?? should-run-as-shell'*'"exit_code":127'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-handoff-bypass-marker","message":{"content":[{"type":"text","text":"HANDOFF BYPASS MARKER HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-handoff-bypass-marker","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-handoff-bypass-marker","is_error":true,"result":"missing marker-bypass host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-handoff-bypass-marker","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-handoff-bypass-marker\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("HANDOFF BYPASS MARKER HOSTEXEC RECEIVED"),
        "{output}"
    );
    assert!(!output.contains("agent_marker"), "{output}");
    assert!(!output.contains("intercepted"), "{output}");
    assert!(
        !output.contains("missing marker-bypass host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_zsh_approved_shell_handoff_bypasses_marker_intercepts() {
    let home = temp_shell_home("cosh-tui-zsh-handoff-bypass-marker");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-zsh-handoff-bypass-marker","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-zsh-handoff-bypass-marker*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-zsh-handoff-bypass-marker","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"?? should-run-as-zsh-shell"},"tool_use_id":"toolu-zsh-handoff-bypass-marker"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'?? should-run-as-zsh-shell'*'"exit_code":127'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-zsh-handoff-bypass-marker","message":{"content":[{"type":"text","text":"ZSH HANDOFF BYPASS MARKER HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-zsh-handoff-bypass-marker","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-zsh-handoff-bypass-marker","is_error":true,"result":"missing zsh marker-bypass host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-zsh-handoff-bypass-marker","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &["--shell", "zsh"],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval trust confirm\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-zsh-handoff-bypass-marker\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("ZSH HANDOFF BYPASS MARKER HOSTEXEC RECEIVED"),
        "{output}"
    );
    assert!(!output.contains("agent_marker"), "{output}");
    assert!(!output.contains("intercepted"), "{output}");
    assert!(
        !output.contains("missing zsh marker-bypass host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_approved_shell_handoff_wrapper_does_not_leak() {
    let home = temp_shell_home("cosh-tui-handoff-wrapper-leak");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-handoff-wrapper-leak","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-handoff-wrapper-leak*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-handoff-wrapper-leak","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"printf wrapper-visible"},"tool_use_id":"toolu-handoff-wrapper-leak"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'printf wrapper-visible'*'wrapper-visible'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-handoff-wrapper-leak","message":{"content":[{"type":"text","text":"HANDOFF WRAPPER LEAK HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-handoff-wrapper-leak","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-handoff-wrapper-leak","is_error":true,"result":"missing wrapper-leak host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-handoff-wrapper-leak","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-handoff-wrapper-leak\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("wrapper-visible"), "{output}");
    assert!(
        output.contains("HANDOFF WRAPPER LEAK HOSTEXEC RECEIVED"),
        "{output}"
    );
    assert!(!output.contains("COSH_SHELL_HANDOFF_BYPASS"), "{output}");
    assert!(
        !output.contains("missing wrapper-leak host_executed_shell result"),
        "{output}"
    );
}
