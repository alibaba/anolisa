use super::*;

#[test]
fn raw_cli_cosh_tui_approval_mode_argv_maps_to_cosh_tui_modes() {
    for (label, mode_input, expected_mode) in [
        ("recommend", "/mode approval recommend\n", "strict"),
        ("auto", "/mode approval auto\n", "auto"),
        ("trust", "/mode approval trust confirm\n", "trust"),
    ] {
        let home = temp_shell_home(&format!("cosh-tui-mode-argv-{label}"));
        let bin_dir = home.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let cosh_tui_path = bin_dir.join("cosh-tui");
        write_executable(
            &cosh_tui_path,
            r#"#!/bin/sh
mode=missing
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--approval-mode" ]; then
    shift
    mode="${1:-missing}"
    break
  fi
  shift
done
if [ "$mode" = "strict" ]; then
  printf '{"type":"assistant","session_id":"sess-cosh-tui-mode-argv","message":{"content":[{"type":"text","text":"ARGV_APPROVAL_MODE=%s"}]}}\n' "$mode"
  printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-mode-argv","is_error":false,"result":"done"}'
  exit 0
fi
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-mode-argv","model":"cosh-tui-test"}'
read -r user_message
printf '{"type":"assistant","session_id":"sess-cosh-tui-mode-argv","message":{"content":[{"type":"text","text":"ARGV_APPROVAL_MODE=%s"}]}}\n' "$mode"
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-mode-argv","is_error":false,"result":"done"}'
"#,
        );
        let home_str = home.to_string_lossy().to_string();
        let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
        let query = format!("?? cosh-tui-mode-argv-{label}\n");
        let output = run_raw_cli_with_args_env_and_delayed_input(
            "cosh-tui",
            &[],
            &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
            vec![
                (mode_input.as_bytes().to_vec(), Duration::ZERO),
                (query.into_bytes(), Duration::from_millis(500)),
                (b"exit\n".to_vec(), Duration::from_millis(500)),
            ],
        );
        let _ = fs::remove_dir_all(&home);

        assert!(
            output.contains(&format!("ARGV_APPROVAL_MODE={expected_mode}")),
            "{output}"
        );
        assert!(!output.contains("ARGV_APPROVAL_MODE=missing"), "{output}");
    }
}

#[test]
fn raw_cli_cosh_tui_trust_without_confirm_does_not_enable_trust() {
    let home = temp_shell_home("cosh-tui-trust-without-confirm");
    let marker = home.join("should-not-exist");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    let command = format!("touch {}", marker.display());
    let script = format!(
        r#"#!/bin/sh
read -r init
printf '%s\n' '{{"type":"control_response","response":{{"subtype":"success","request_id":"init-1","response":{{"subtype":"initialize","capabilities":{{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}}}}}'
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-cosh-tui-trust-unconfirmed","model":"cosh-tui-test"}}'
read -r user_message
case "$user_message" in
  *cosh-tui-trust-without-confirm*)
    printf '%s\n' '{{"type":"control_request","request_id":"ctrl-trust-unconfirmed","request":{{"subtype":"can_use_tool","tool_name":"shell","input":{{"command":"{command}"}},"tool_use_id":"toolu-trust-unconfirmed"}}}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"deny"'*)
          printf '%s\n' '{{"type":"assistant","session_id":"sess-cosh-tui-trust-unconfirmed","message":{{"content":[{{"type":"text","text":"TRUST UNCONFIRMED DENIED"}}]}}}}'
          printf '%s\n' '{{"type":"result","subtype":"success","session_id":"sess-cosh-tui-trust-unconfirmed","is_error":false,"result":"done"}}'
          exit 0
          ;;
        *'"behavior":"host_executed_shell"'*|*'"behavior":"allow"'*)
          printf '%s\n' '{{"type":"result","subtype":"error","session_id":"sess-cosh-tui-trust-unconfirmed","is_error":true,"result":"trust unconfirmed unexpectedly approved"}}'
          exit 1
          ;;
      esac
    fi
    printf '%s\n' '{{"type":"result","subtype":"error","session_id":"sess-cosh-tui-trust-unconfirmed","is_error":true,"result":"missing trust unconfirmed denial"}}'
    exit 1
    ;;
esac
printf '%s\n' '{{"type":"result","subtype":"success","session_id":"sess-cosh-tui-trust-unconfirmed","is_error":false,"result":"ignored"}}'
"#
    );
    write_executable(&cosh_tui_path, &script);
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval trust\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-trust-without-confirm\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\x1b".to_vec(), Duration::from_millis(1_200)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("Trust confirmation required"), "{output}");
    assert!(
        output.contains("Run /mode approval trust confirm"),
        "{output}"
    );
    assert!(output.contains("Approval required"), "{output}");
    assert!(output.contains(&command), "{output}");
    assert!(!output.contains("Mode set to trust."), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");
    assert!(!output.contains("Trusted req-1"), "{output}");
    assert!(
        !output.contains("trust unconfirmed unexpectedly approved"),
        "{output}"
    );
    assert!(!marker.exists(), "{output}");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn raw_cli_cosh_tui_non_shell_permission_passes_allow_only() {
    let home = temp_shell_home("cosh-tui-non-shell-pass-through");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-non-shell-pass-through","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-write-pass-through*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-write","request":{"subtype":"can_use_tool","tool_name":"write_file","input":{"file_path":"/tmp/cosh-tui-provider-smoke.txt","content":"ok"},"tool_use_id":"toolu-cosh-tui-write"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ctrl-cosh-tui-write"'*'"behavior":"allow"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-non-shell-pass-through","message":{"content":[{"type":"text","text":"Cosh-tui non-shell write permission allowed through provider control protocol."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-non-shell-pass-through","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-non-shell-pass-through","is_error":true,"result":"missing non-shell allow response"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-non-shell-pass-through","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-write-pass-through\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit 0\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approval required"), "{output}");
    assert!(output.contains("Subject: Write"), "{output}");
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(
        output.contains(
            "Cosh-tui non-shell write permission allowed through provider control protocol."
        ),
        "{output}"
    );
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
    assert!(
        !output.contains("missing non-shell allow response"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_non_shell_permission_deny_does_not_write_or_host_execute() {
    let denied_path =
        std::env::temp_dir().join(format!("cosh-tui-denied-write-{}", std::process::id()));
    let _ = fs::remove_file(&denied_path);
    let denied_path_str = denied_path.to_string_lossy().to_string();
    let home = temp_shell_home("cosh-tui-non-shell-deny");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    let script = format!(
        r#"#!/bin/sh
read -r init
printf '%s\n' '{{"type":"control_response","response":{{"subtype":"success","request_id":"init-1","response":{{"subtype":"initialize","capabilities":{{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}}}}}'
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-cosh-tui-non-shell-deny","model":"cosh-tui-test"}}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-write-deny*)
    printf '%s\n' '{{"type":"control_request","request_id":"ctrl-cosh-tui-write-deny","request":{{"subtype":"can_use_tool","tool_name":"write_file","input":{{"file_path":"{denied_path}","content":"denied"}},"tool_use_id":"toolu-cosh-tui-write-deny"}}}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ctrl-cosh-tui-write-deny"'*'"behavior":"deny"'*)
          printf '%s\n' '{{"type":"assistant","session_id":"sess-cosh-tui-non-shell-deny","message":{{"content":[{{"type":"text","text":"Cosh-tui non-shell write permission denied without host execution."}}]}}}}'
          printf '%s\n' '{{"type":"result","subtype":"success","session_id":"sess-cosh-tui-non-shell-deny","is_error":false,"result":"done"}}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{{"type":"result","subtype":"error","session_id":"sess-cosh-tui-non-shell-deny","is_error":true,"result":"missing non-shell deny response"}}'
    exit 1
    ;;
esac
printf '%s\n' '{{"type":"result","subtype":"success","session_id":"sess-cosh-tui-non-shell-deny","is_error":false,"result":"ignored"}}'
"#,
        denied_path = denied_path_str
    );
    write_executable(&cosh_tui_path, &script);
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-provider-write-deny\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\x1b[C\x1b[C\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit 0\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approval required"), "{output}");
    assert!(output.contains("Subject: Write"), "{output}");
    assert!(output.contains("Denied req-1"), "{output}");
    assert!(
        output.contains("Cosh-tui non-shell write permission denied without host execution."),
        "{output}"
    );
    assert!(!denied_path.exists(), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
    assert!(
        !output.contains("missing non-shell deny response"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    let _ = fs::remove_file(&denied_path);
}
