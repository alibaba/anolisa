use super::*;

#[test]
fn raw_cli_slash_after_failed_command_invokes_adapter() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\n/explain last error\necho after-explain\nexit 0\n",
        &[("COSH_SHELL_LANG", "en-US")],
    );

    assert_agent_loading_visible(&output);
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(output.contains("Command failed:"), "{output}");
    assert_inline_before_followup(&output, "Thinking...", "The command");
    assert_inline_before_followup(&output, "The command", "after-explain");
}

#[test]
fn raw_cli_smart_usage_help_failure_does_not_start_agent() {
    let fixture = temp_shell_home("usage-help-smart");
    let bin_dir = fixture.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("demo-usage"),
        "#!/bin/sh\n\
         echo \"error: unexpected argument '$1'\" >&2\n\
         echo \"Usage: demo-usage [OPTIONS]\" >&2\n\
         echo \"Try 'demo-usage --help' for more information.\" >&2\n\
         exit 2\n",
    );
    let path = format!(
        "{}:{}",
        bin_dir.to_string_lossy(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = run_raw_cli_with_env(
        "fake",
        "demo-usage --bad\necho after-usage\nexit\n",
        &[("COSH_SHELL_LANG", "en-US"), ("PATH", path.as_str())],
    );
    let _ = fs::remove_dir_all(&fixture);

    assert!(output.contains("Usage: demo-usage [OPTIONS]"), "{output}");
    assert!(output.contains("after-usage"), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(
        !output.contains("The command demo-usage --bad failed"),
        "{output}"
    );
    assert!(!output.contains("Command failed"), "{output}");
}

#[test]
fn raw_cli_explain_after_usage_help_failure_invokes_adapter() {
    let fixture = temp_shell_home("usage-help-explain");
    let bin_dir = fixture.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("demo-usage"),
        "#!/bin/sh\n\
         echo \"error: unexpected argument '$1'\" >&2\n\
         echo \"Usage: demo-usage [OPTIONS]\" >&2\n\
         echo \"Try 'demo-usage --help' for more information.\" >&2\n\
         exit 2\n",
    );
    let path = format!(
        "{}:{}",
        bin_dir.to_string_lossy(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = run_raw_cli_with_env(
        "fake",
        "demo-usage --bad\n/explain last error\necho after-explain\nexit\n",
        &[("COSH_SHELL_LANG", "en-US"), ("PATH", path.as_str())],
    );
    let _ = fs::remove_dir_all(&fixture);

    assert_agent_loading_visible(&output);
    assert!(
        output.contains("The command demo-usage --bad failed"),
        "{output}"
    );
    assert_inline_before_followup(&output, "The command", "after-explain");
}

#[test]
fn raw_cli_smart_command_not_found_renders_action_card_without_agent() {
    let output = run_raw_cli_with_env(
        "fake",
        "cosh_missing_command_for_failure_policy\necho after-missing\nexit\n",
        &[("COSH_SHELL_LANG", "en-US")],
    );

    assert!(output.contains("Command failed"), "{output}");
    assert!(output.contains("[Analyze] [Dismiss] [Details]"), "{output}");
    assert!(output.contains("after-missing"), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(
        !output.contains("The command cosh_missing_command_for_failure_policy failed"),
        "{output}"
    );
}

#[test]
fn raw_cli_clear_cancels_pending_failed_command_analysis() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\n/clear\n/explain last error\necho after-clear\nexit 0\n",
        &[("COSH_SHELL_ANALYSIS_MODE", "auto")],
    );

    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert_eq!(
        count_occurrences(&output, "The command ls /path/that/does/not/exist failed"),
        1,
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
    assert!(output.contains("after-clear"));
}

#[test]
fn raw_cli_shell_cancels_pending_failed_command_analysis() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\n/shell\n/explain last error\necho after-shell\nexit 0\n",
        &[("COSH_SHELL_ANALYSIS_MODE", "auto")],
    );

    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert_eq!(
        count_occurrences(&output, "The command ls /path/that/does/not/exist failed"),
        1,
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
    assert!(output.contains("after-shell"));
}

#[test]
fn raw_cli_natural_language_keeps_later_failed_command_auto_analysis() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_ANALYSIS_MODE", "auto")],
        vec![
            ("\u{4f60}\u{597d}\n".as_bytes().to_vec(), Duration::ZERO),
            (
                b"ls /path/that/does/not/exist\n".to_vec(),
                Duration::from_millis(300),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert_agent_loading_visible(&output);
    assert!(output.contains("Received shell prompt request"));
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed:"), "{output}");
}

#[test]
fn raw_cli_natural_language_includes_recent_failed_command_fact_without_hook_hints() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         please show context\n\
         exit\n",
    );

    assert!(
        output.contains("Recent context visible to Agent"),
        "{output}"
    );
    let compact = compact_terminal_words(&output);
    assert!(
        compact.contains("Runtime context hints visible to Agent: <none>"),
        "{output}"
    );
    assert!(
        compact.contains("command=ls /path/that/does/not/exist"),
        "{output}"
    );
    assert!(
        compact.contains("output_id=terminal-output://raw-session-"),
        "{output}"
    );
    assert!(compact.contains("/cmd-1"), "{output}");
    assert!(
        !compact.contains("output_id=terminal-output://raw-session/cmd-1"),
        "{output}"
    );
    assert!(!output.contains("hook-cmd-"), "{output}");
    assert!(!output.contains("ref="), "{output}");
    assert!(
        !output.contains("No command ran; Agent actions still require governance."),
        "{output}"
    );
}

#[test]
fn raw_cli_natural_language_after_failure_keeps_failed_command_analysis() {
    let input = "ls /path/that/does/not/exist\n\u{4f60}\u{597d}\nexit 0\n";
    let output = run_raw_cli_with_env(
        "fake",
        input,
        &[
            ("COSH_SHELL_LANG", "en-US"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
    );

    assert_agent_loading_visible(&output);
    assert!(output.contains("Received shell prompt request: \u{4f60}\u{597d}"));
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed:"), "{output}");
}

#[test]
fn raw_cli_failed_command_guidance_appears_before_next_prompt() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("COSH_SHELL_LANG", "en-US"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
        vec![
            (b"ls ccc\n".to_vec(), Duration::ZERO),
            (b"exit 0\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("ls: ccc: No such file or directory"));
    assert!(output.contains("The command ls ccc failed with exit code 1."));
    assert_inline_before_followup(
        &output,
        "The command ls ccc failed with exit code 1.",
        "exit 0",
    );
    assert!(!output.contains("Command failed:"), "{output}");
    assert!(!output.contains("Agent not called"));
    assert!(!output.contains("suggestion: show a short explanation"));
    assert!(!output.contains("`exit` exited with code"));
    assert!(!output.contains("The command exit failed"));
    assert!(!output.contains("Approval not found"), "{output}");
}

#[test]
fn raw_cli_repeated_failed_command_skips_without_auto_analyzed_notice() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_ANALYSIS_MODE", "auto")],
        vec![
            (b"ls ccc\n".to_vec(), Duration::ZERO),
            (b"ls ccc\n".to_vec(), Duration::from_millis(800)),
            (
                b"echo after-repeat\nexit\n".to_vec(),
                Duration::from_millis(800),
            ),
        ],
    );

    assert_eq!(
        count_occurrences(&output, "Analysis skipped"),
        1,
        "{output}"
    );
    assert!(
        output.contains("skipped repeated failure analysis for `ls ccc`"),
        "{output}"
    );
    assert_eq!(
        count_occurrences(&output, "The command ls ccc failed with exit code 1."),
        1,
        "{output}"
    );
    assert!(output.contains("after-repeat"), "{output}");
    assert!(!output.contains("[Analyze] [Ignore]"), "{output}");
}

#[test]
fn raw_cli_zh_repeated_failed_command_uses_localized_notices() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("COSH_SHELL_LANG", "zh-CN"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
        vec![
            (b"ls ccc\n".to_vec(), Duration::ZERO),
            (b"ls ccc\n".to_vec(), Duration::from_millis(800)),
            (
                b"echo after-zh-repeat\nexit\n".to_vec(),
                Duration::from_millis(800),
            ),
        ],
    );

    assert_eq!(count_occurrences(&output, "已跳过分析"), 1, "{output}");
    assert!(
        output.contains("已跳过 `ls ccc` 的重复失败分析"),
        "{output}"
    );
    assert!(output.contains("Agent 回复"), "{output}");
    assert!(output.contains("The command ls ccc failed with exit code 1."));
    assert!(output.contains("after-zh-repeat"), "{output}");
    assert!(!output.contains("bash: ls ccc"), "{output}");
}

#[test]
fn raw_cli_hook_consultation_uses_zh_language_env() {
    let fixture = temp_shell_home("hook-consultation-zh");
    let bin_dir = fixture.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("free"),
        "#!/bin/sh\ncat <<'EOF'\n              total        used        free      shared  buff/cache   available\nMem:          32768       30200         380          16        2188        1400\nSwap:          8192        4096        4096\nEOF\n",
    );
    let path = format!(
        "{}:{}",
        bin_dir.to_string_lossy(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN"), ("PATH", path.as_str())],
        vec![
            (b"free -m\n".to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(1200)),
        ],
    );

    assert!(output.contains("[分析] [忽略]"), "{output}");
    assert!(output.contains("Available memory is low"), "{output}");
    assert!(output.contains("发现:"), "{output}");
    assert!(output.contains("建议动作:"), "{output}");
    assert!(output.contains("Use memory-analysis"), "{output}");
    assert!(!output.contains("Hook: memory-pressure"), "{output}");
    assert!(
        !output.contains("置信度: medium; 原因: allowed"),
        "{output}"
    );
    assert!(!output.contains("Confidence:"), "{output}");
    assert!(!output.contains("reason:"), "{output}");
    assert!(!output.contains("[Analyze] [Ignore]"), "{output}");
    assert!(!output.contains("bash: free -m"), "{output}");
}

#[test]
fn raw_cli_repeated_ps_dash_aux_failure_is_stable_and_keeps_prompt() {
    let fixture = temp_shell_home("ps-dash-aux");
    let bin_dir = fixture.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let ps_path = bin_dir.join("ps");
    fs::write(
        &ps_path,
        "#!/bin/sh\nif [ \"$1\" = \"-aux\" ]; then\n  echo \"ps: No user named 'x'\" >&2\n  exit 1\nfi\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&ps_path, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.to_string_lossy(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("PATH", path.as_str()),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
        vec![
            (b"ps -aux\n".to_vec(), Duration::ZERO),
            (b"ps -aux\n".to_vec(), Duration::from_millis(1200)),
            (
                b"echo after-ps-repeat\nexit\n".to_vec(),
                Duration::from_millis(1200),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&fixture);

    assert_eq!(
        count_occurrences(&output, "Analysis skipped"),
        1,
        "{output}"
    );
    assert!(
        output.contains("skipped repeated failure analysis for `ps -aux`"),
        "{output}"
    );
    assert_eq!(
        count_occurrences(&output, "The command ps -aux failed with exit code 1."),
        1,
        "{output}"
    );
    assert!(output.contains("after-ps-repeat"), "{output}");
    assert_inline_before_followup(
        &output,
        "The command ps -aux failed with exit code 1.",
        "after-ps-repeat",
    );
    assert!(!output.contains("[Analyze] [Ignore]"), "{output}");
    assert!(!output.contains("Approval not found"), "{output}");
}

#[test]
fn raw_cli_tail_follow_ctrl_c_does_not_start_agent_analysis() {
    let output = run_raw_cli_with_input(
        "fake",
        "bash -c 'tail -f /dev/null & BGPID=$!; sleep 0.2; kill $BGPID; wait $BGPID 2>/dev/null'\necho after-tail-follow\nexit\n",
    );

    assert!(output.contains("after-tail-follow"), "{output}");
    assert!(!output.contains("Command hook"), "{output}");
    assert!(!output.contains("Command result finding"), "{output}");
}

#[test]
fn raw_cli_inline_guidance_works_with_fake_adapter() {
    let output = run_raw_cli_with_envs(
        "fake",
        &[
            ("COSH_SHELL_LANG", "en-US"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
    );

    assert!(output.contains("Thinking..."));
    assert!(!output.contains("Agent status"));
    assert!(!output.contains("Phase: analyzing"));
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert_inline_before_followup(&output, "The command", "after-inline");
}

#[test]
fn raw_cli_zsh_failed_command_auto_hook_restores_prompt_without_consultation_card() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_zsh_home("failed-hook-prompt");
    fs::write(home.join(".zshrc"), "PROMPT='ZPROMPT> '\nRPROMPT=''\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_ANALYSIS_MODE", "auto"),
        ],
        vec![
            (b"ls ccc\n".to_vec(), Duration::ZERO),
            (
                b"echo after-hook\nexit\n".to_vec(),
                Duration::from_millis(1200),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert_agent_loading_visible(&output);
    assert!(!output.contains("[Analyze] [Ignore]"), "{output}");
    assert!(output.contains("The command ls ccc failed with exit code 1."));
    assert!(output.contains("after-hook"), "{output}");
    assert!(
        count_occurrences_between(
            &output,
            "The command ls ccc failed with exit code 1.",
            "echo after-hook",
            "ZPROMPT> "
        ) >= 1,
        "{output}"
    );
}
