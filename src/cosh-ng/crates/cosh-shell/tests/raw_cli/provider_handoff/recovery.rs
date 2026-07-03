use super::*;

#[test]
fn raw_cli_shell_handoff_resume_timeout_retries_without_timeout_card() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider resume timeout shell trigger resume timeout\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (b"exit 0\n".to_vec(), Duration::from_millis(6_000)),
        ],
    );

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ ssh -V"), "{output}");
    assert!(
        output.contains("Command result analysis for req-1: foreground shell evidence received"),
        "{output}"
    );
    assert!(
        output.contains("Using a fresh provider turn for shell evidence recovery."),
        "{output}"
    );
    assert!(
        output.contains("Provider session continuity may be degraded."),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(
        !output.contains("No provider response within 20s"),
        "{output}"
    );
}

#[test]
fn raw_cli_shell_handoff_resume_timeout_renders_structured_context_before_recovery_notice() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider resume timeout shell structured before recovery\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (b"exit 0\n".to_vec(), Duration::from_millis(6_000)),
        ],
    );

    assert!(
        output.contains("Approved req-1") || output.contains("Auto-approved req-1"),
        "{output}"
    );
    assert!(
        output.contains("$ printf structured-before-recovery"),
        "{output}"
    );
    assert_ordered(
        &output,
        &[
            "structured-before-recovery",
            "Using a fresh provider turn for shell evidence recovery.",
            "Command result analysis for req-1: foreground shell evidence received",
        ],
    );
    assert_eq!(
        count_occurrences(
            &output,
            "Using a fresh provider turn for shell evidence recovery."
        ),
        1,
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(
        !output.contains("No provider response within 20s"),
        "{output}"
    );
}

#[test]
fn raw_cli_shell_handoff_recovery_uses_zh_language_env() {
    let home = temp_shell_home("handoff-recovery-zh");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("ssh"),
        "#!/bin/sh\nprintf 'OpenSSH_fake_for_recovery\\n'\n",
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("COSH_SHELL_LANG", "zh-CN"),
            ("HOME", &home_str),
            ("PATH", &path),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider resume timeout shell trigger resume timeout\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (b"exit 0\n".to_vec(), Duration::from_millis(6_000)),
        ],
    );

    assert!(output.contains("已批准 req-1"), "{output}");
    assert!(output.contains("$ ssh -V"), "{output}");
    assert!(output.contains("OpenSSH_fake_for_recovery"), "{output}");
    assert!(output.contains("Agent 恢复"), "{output}");
    assert!(
        output.contains("正在使用新的 provider 轮次恢复 shell evidence。"),
        "{output}"
    );
    assert!(output.contains("Provider 会话连续性可能降低。"), "{output}");
    assert!(
        !output.contains("Using a fresh provider turn for shell evidence recovery."),
        "{output}"
    );
    assert!(
        !output.contains("Provider session continuity may be degraded."),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(
        !output.contains("No provider response within 20s"),
        "{output}"
    );
}

#[test]
fn raw_cli_zh_provider_timeout_drops_extra_queued_requests() {
    let home = temp_shell_home("qwen-timeout-dropped-queue-zh");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *first-timeout*)
      sleep 30
      exit 0
      ;;
    *queued-one*)
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-timeout-queue","model":"qwen-test"}'
      printf '%s\n' '{"type":"assistant","session_id":"sess-timeout-queue","message":{"content":[{"type":"text","text":"Queued request one completed."}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-timeout-queue","is_error":false,"result":"done"}'
      exit 0
      ;;
    *queued-two*)
      printf '%s\n' '{"type":"assistant","session_id":"sess-timeout-queue","message":{"content":[{"type":"text","text":"Queued request two should have been dropped."}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-timeout-queue","is_error":false,"result":"done"}'
      exit 0
      ;;
  esac
done
sleep 30
exit 0
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
            ("COSH_SHELL_LANG", "zh-CN"),
            ("COSH_AGENT_START_TIMEOUT_SECS", "1"),
        ],
        vec![
            (b"?? first-timeout\n".to_vec(), Duration::ZERO),
            (b"?? queued-one\n".to_vec(), Duration::from_millis(100)),
            (b"?? queued-two\n".to_vec(), Duration::from_millis(100)),
            (b"exit 0\n".to_vec(), Duration::from_millis(2_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("provider 超时后已跳过 1 个排队请求"),
        "{output}"
    );
    assert!(output.contains("Queued request one completed."), "{output}");
    assert!(
        !output.contains("Queued request two should have been dropped."),
        "{output}"
    );
    assert!(
        !output.contains("1 queued requests skipped after provider timeout"),
        "{output}"
    );
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("bash: ??"), "{output}");
}
