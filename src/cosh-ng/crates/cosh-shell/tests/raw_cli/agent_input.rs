use super::*;

#[test]
fn raw_cli_zsh_shell_arg_intercepts_fragmented_agent_marker() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"?? zsh ".to_vec(), Duration::ZERO),
            (b"fragmented agent\n".to_vec(), Duration::from_millis(50)),
            (
                b"echo after-zsh-agent\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert_agent_loading_visible(&output);
    assert!(
        output.contains("Received shell prompt request: ?? zsh fragmented agent"),
        "{output}"
    );
    assert!(output.contains("after-zsh-agent"), "{output}");
    assert!(!output.contains("zsh: command not found: ??"), "{output}");
    assert!(!output.contains("\x1b]1337;COSH;"), "{output}");
}

#[test]
fn raw_cli_zsh_shell_arg_intercepts_fragmented_natural_language() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        vec![
            (b"\xe4\xbd".to_vec(), Duration::ZERO),
            (b"\xa0\xe5\xa5\xbd\n".to_vec(), Duration::from_millis(50)),
            (
                b"echo after-zsh-natural\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("\u{4f60}\u{597d}"), "{output}");
    assert_agent_loading_visible(&output);
    assert!(
        output.contains("Received shell prompt request: \u{4f60}\u{597d}"),
        "{output}"
    );
    assert!(output.contains("after-zsh-natural"), "{output}");
    assert!(
        !output.contains("zsh: command not found: \u{4f60}\u{597d}"),
        "{output}"
    );
}

#[test]
fn raw_cli_natural_language_omits_recent_command_facts_by_default() {
    let output = run_raw_cli_with_input(
        "fake",
        "echo shell-context-ok\n\
         please show context\n\
         exit\n",
    );

    assert!(output.contains("shell-context-ok"), "{output}");
    assert!(
        output.contains("Recent context visible to Agent: <none>"),
        "{output}"
    );
    let no_wrap: String = output.replace('│', "");
    assert!(
        !no_wrap.contains("command=echo shell-context-ok"),
        "{output}"
    );
    assert!(
        !no_wrap.contains("output_id=terminal-output://raw-session-"),
        "{output}"
    );
    assert!(!no_wrap.contains("/cmd-1"), "{output}");
    assert!(
        !no_wrap.contains("output_id=terminal-output://raw-session/cmd-1"),
        "{output}"
    );
    assert!(!no_wrap.contains("command=exit"), "{output}");
    assert!(!no_wrap.contains("preview:"), "{output}");
    assert!(!output.contains("ref="), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
}

#[test]
fn raw_cli_delays_agent_output_while_foreground_command_is_active() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? hold test slow agent\n".to_vec(), Duration::ZERO),
            (
                b"sleep 0.3; echo after-foreground\n".to_vec(),
                Duration::from_millis(200),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(3_500)),
        ],
    );

    assert!(output.contains("Thinking..."), "{output}");
    assert!(output.contains("after-foreground"), "{output}");
    assert!(
        output.contains("Slow fake response for: ?? hold test slow agent"),
        "{output}"
    );
    assert_inline_before_followup(
        &output,
        "after-foreground",
        "Slow fake response for: ?? hold test slow agent",
    );
}

#[test]
fn raw_cli_agent_marker_invokes_adapter_without_failed_command() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"?? check current directory\n".to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );

    assert!(output.contains("Thinking..."));
    assert!(output.contains("Received shell prompt request: ?? check current directory"));
    assert!(!output.contains("command exited with code"));
    assert_no_prompt_between(&output, "Thinking...", "Received shell prompt request");
}

#[test]
fn raw_cli_zh_natural_language_intercept_skips_redundant_notice() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            ("帮我看看当前目录\n".as_bytes().to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );

    assert!(!output.contains("AI 请求"), "{output}");
    assert!(!output.contains("正在把输入交给 Agent"), "{output}");
    assert!(
        !output.contains("该输入已在进入 Bash 前被拦截。"),
        "{output}"
    );
    assert!(output.contains("正在思考..."), "{output}");
    assert!(
        output.contains("已收到 Shell 提示请求：帮我看看当前目录"),
        "{output}"
    );
    assert!(
        !output.contains("Received shell prompt request"),
        "{output}"
    );
    assert!(!output.contains("bash: 帮我看看当前目录"), "{output}");
}

#[test]
fn raw_cli_zsh_agent_response_restores_prompt_without_empty_command() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_zsh_home("agent-prompt");
    fs::write(home.join(".zshrc"), "PROMPT='ZPROMPT> '\nRPROMPT=''\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (b"?? zsh prompt smoke\n".to_vec(), Duration::ZERO),
            (
                b"echo after-agent\nexit\n".to_vec(),
                Duration::from_millis(1200),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Received shell prompt request: ?? zsh prompt smoke"),
        "{output}"
    );
    assert!(output.contains("after-agent"), "{output}");
    assert!(count_occurrences(&output, "ZPROMPT> ") >= 2, "{output}");
    assert!(
        count_occurrences_between(
            &output,
            "Received shell prompt request: ?? zsh prompt smoke",
            "echo after-agent",
            "ZPROMPT> "
        ) >= 1,
        "{output}"
    );
    assert_no_standalone_percent_line(&output);
}

#[test]
fn raw_cli_bash_agent_prompt_restore_does_not_duplicate_prompt() {
    let home = temp_shell_home("agent-prompt-bash");
    fs::write(home.join(".bashrc"), "PS1='BPROMPT> '\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "bash"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (b"?? bash prompt smoke\n".to_vec(), Duration::ZERO),
            (
                b"echo after-agent\nexit\n".to_vec(),
                Duration::from_millis(1200),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Received shell prompt request: ?? bash prompt smoke"),
        "{output}"
    );
    assert!(output.contains("after-agent"), "{output}");
    let prompt_count = count_occurrences_between(
        &output,
        "Received shell prompt request: ?? bash prompt smoke",
        "echo after-agent",
        "BPROMPT> ",
    );
    assert!(
        (1..=2).contains(&prompt_count),
        "prompt_count={prompt_count}\n{output}"
    );
}

#[test]
fn raw_cli_zsh_agent_prompt_restore_suppresses_partial_line_marker() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_zsh_home("agent-prompt-sp");
    fs::write(
        home.join(".zshrc"),
        "PROMPT='ZPROMPT> '\n\
         RPROMPT=''\n\
         autoload -Uz add-zsh-hook\n\
         _cosh_test_force_prompt_sp() { setopt PROMPT_SP PROMPT_CR; }\n\
         add-zsh-hook precmd _cosh_test_force_prompt_sp\n",
    )
    .unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (b"?? zsh prompt sp smoke\n".to_vec(), Duration::ZERO),
            (
                b"echo after-agent\nexit\n".to_vec(),
                Duration::from_millis(1200),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Received shell prompt request: ?? zsh prompt sp smoke"),
        "{output}"
    );
    assert!(output.contains("after-agent"), "{output}");
    assert!(count_occurrences(&output, "ZPROMPT> ") >= 2, "{output}");
    assert_no_standalone_percent_line(&output);
}

#[test]
fn raw_cli_zsh_shell_marker_agent_response_does_not_duplicate_prompt() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_shell_home("agent-shell-marker-zsh");
    fs::write(home.join(".zshrc"), "PROMPT='ZPROMPT> '\nRPROMPT=''\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            ("\u{4f60}\u{597d}\n".as_bytes().to_vec(), Duration::ZERO),
            (
                b"echo after-agent\nexit\n".to_vec(),
                Duration::from_millis(1200),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Received shell prompt request: \u{4f60}\u{597d}"),
        "{output}"
    );
    assert!(output.contains("after-agent"), "{output}");
    assert_eq!(count_occurrences(&output, "ZPROMPT> "), 3, "{output}");
    assert_eq!(
        count_occurrences_between(
            &output,
            "Received shell prompt request: \u{4f60}\u{597d}",
            "echo after-agent",
            "ZPROMPT> "
        ),
        1,
        "{output}"
    );
    assert_no_standalone_percent_line(&output);
}

#[test]
fn raw_cli_bash_shell_marker_agent_response_does_not_duplicate_prompt() {
    let home = temp_shell_home("agent-shell-marker-bash");
    fs::write(home.join(".bashrc"), "PS1='BPROMPT> '\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "bash"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            ("\u{4f60}\u{597d}\n".as_bytes().to_vec(), Duration::ZERO),
            (
                b"echo after-agent\nexit\n".to_vec(),
                Duration::from_millis(1200),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Received shell prompt request: \u{4f60}\u{597d}"),
        "{output}"
    );
    assert!(output.contains("after-agent"), "{output}");
    assert_eq!(count_occurrences(&output, "BPROMPT> "), 3, "{output}");
    assert_eq!(
        count_occurrences_between(
            &output,
            "Received shell prompt request: \u{4f60}\u{597d}",
            "echo after-agent",
            "BPROMPT> "
        ),
        1,
        "{output}"
    );
}

#[test]
fn raw_cli_empty_enter_and_ctrl_c_do_not_start_agent() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(50)),
            (b"\nexit 0\n".to_vec(), Duration::from_millis(50)),
        ],
    );

    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("Command failed:"), "{output}");
    assert!(!output.contains("Agent status"), "{output}");
    assert!(output.contains("exit 0"), "{output}");
}

#[test]
fn raw_cli_empty_enter_after_agent_response_does_not_retrigger() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            ("\u{4f60}\u{597d}\n".as_bytes().to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(50)),
        ],
    );

    assert_eq!(agent_loading_count(&output), 1, "{output}");
    assert_eq!(
        count_occurrences(&output, "Received shell prompt request"),
        1,
        "{output}"
    );
    let response_pos = output
        .find("Received shell prompt request")
        .expect("agent response");
    let prompt_after_response = output[response_pos..]
        .find("cosh-osc$")
        .expect("prompt after agent response");
    assert!(prompt_after_response > 0, "{output}");
}

#[test]
fn raw_cli_non_ascii_agent_input_echoes_before_intercept() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            ("\u{4f60}".as_bytes().to_vec(), Duration::ZERO),
            ("\u{597d}".as_bytes().to_vec(), Duration::from_millis(50)),
            (b"\n".to_vec(), Duration::from_millis(50)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("cosh-osc$ \u{4f60}\u{597d}"), "{output}");
    assert_eq!(
        count_occurrences(&output, "\n\u{4f60}\u{597d}"),
        0,
        "{output}"
    );
    assert!(
        output.contains("Received shell prompt request: \u{4f60}\u{597d}"),
        "{output}"
    );
    assert!(output.contains("cosh-osc$ exit"), "{output}");
    assert!(!output.contains("bash: \u{4f60}\u{597d}"), "{output}");
}

#[test]
fn raw_cli_non_ascii_candidate_input_supports_backspace() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            ("\u{4f60}".as_bytes().to_vec(), Duration::ZERO),
            ("\u{597d}".as_bytes().to_vec(), Duration::from_millis(50)),
            (vec![0x7f], Duration::from_millis(50)),
            ("\u{5417}\n".as_bytes().to_vec(), Duration::from_millis(50)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("cosh-osc$ \u{4f60}\u{5417}"), "{output}");
    assert!(
        output.contains("Received shell prompt request: \u{4f60}\u{5417}"),
        "{output}"
    );
    assert!(
        !output.contains("Received shell prompt request: \u{4f60}\u{597d}\u{5417}"),
        "{output}"
    );
    assert!(!output.contains("bash: \u{4f60}\u{5417}"), "{output}");
}
