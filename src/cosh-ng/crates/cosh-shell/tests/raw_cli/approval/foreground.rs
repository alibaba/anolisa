use super::*;

#[test]
fn raw_cli_streaming_tool_approval_renders_before_agent_finishes() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\n".to_vec(), Duration::from_millis(2_500)),
        (b"exit\n".to_vec(), Duration::from_millis(1_000)),
    ]);

    assert!(output.contains("Preparing a streamed tool request before finishing."));
    assert_approval_prompt_visible(&output);
    assert!(output.contains("Subject: Bash"));
    assert!(output.contains("$ git status --short"));
    assert!(output.contains("medium risk"));
    assert!(!output.contains("Subject: tool Bash"));
    assert!(!output.contains("Command: git status --short"));
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("sent to shell"), "{output}");
    assert!(!output.contains("Bash tool - approved"), "{output}");
    assert!(output.contains("$ git status --short"), "{output}");
    assert!(!output.contains("Tool result for request req-1"));
    assert!(!output.contains("Received approved tool result"));
    let approval_marker = if output.contains("Approval req-1") {
        "Approval req-1"
    } else {
        "Approval required"
    };
    assert_inline_before_followup(
        &output,
        "Preparing a streamed tool request before finishing.",
        approval_marker,
    );
    assert!(!output.contains("Analysis continued after the approved command"));
    assert!(!output.contains("stdout captured; [Details]"), "{output}");
    assert!(!output.contains("tool request - approved by user"));
    assert!(!output.contains("Running command"), "{output}");
    assert!(!output.contains("tool-1 tool: executed"));
    assert!(!output.contains("Thinking...Approval"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approved_bash_tool_prints_native_command_and_stdout() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream pwd tool approval\n".to_vec(), Duration::ZERO),
        (b"\n".to_vec(), Duration::from_millis(1_200)),
        (b"exit\n".to_vec(), Duration::from_millis(300)),
    ]);
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert!(output.contains("Preparing a streamed pwd request before finishing."));
    assert_approval_prompt_visible(&output);
    assert!(output.contains("Subject: Bash"), "{output}");
    assert!(output.contains("$ pwd"), "{output}");
    assert!(output.contains(expected_cwd), "{output}");
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("sent to shell"), "{output}");
    assert!(!output.contains("Tool result for request req-1"));
    assert_inline_before_followup(&output, "$ pwd", expected_cwd);
    assert!(!output.contains("Tool called: Bash called"), "{output}");
    assert!(!output.contains("stdout captured; [Details]"), "{output}");
    assert!(!output.contains("Command: pwd"), "{output}");
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approved_bash_tool_streams_delayed_output_before_analysis() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (
            b"?? stream delayed tool approval\n".to_vec(),
            Duration::ZERO,
        ),
        (b"\n".to_vec(), Duration::from_millis(1_200)),
        (b"exit\n".to_vec(), Duration::from_millis(2_600)),
    ]);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Preparing a delayed streamed tool request before finishing."));
    assert_approval_prompt_visible(&output);
    assert!(
        output.contains("$ sleep 1; echo a; sleep 1; echo b"),
        "{output}"
    );
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("sent to shell"), "{output}");
    assert!(normalized.contains("a\nb"), "{output}");
    assert!(
        output.contains("Command result analysis for req-1: foreground shell evidence received"),
        "{output}"
    );
    assert!(!output.contains("Tool result for request req-1"));
    assert!(!output.contains("shell: completed"), "{output}");
    assert_inline_before_followup(&normalized, "$ sleep 1; echo a; sleep 1; echo b", "a\nb");
    assert_inline_before_followup(&normalized, "a\nb", "Command result analysis for req-1");
    assert!(!output.contains("stdout captured; [Details]"), "{output}");
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approved_bash_tool_streams_stderr_to_transcript() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream stderr tool approval\n".to_vec(), Duration::ZERO),
        (b"\n".to_vec(), Duration::from_millis(3_000)),
        (b"\n".to_vec(), Duration::from_millis(2_000)),
        (b"exit\n".to_vec(), Duration::from_millis(4_000)),
    ]);

    assert!(output.contains("Preparing a stderr streamed tool request before finishing."));
    assert_approval_prompt_visible(&output);
    assert!(
        output.contains("$ printf 'out\\n'; printf 'err\\n' >&2"),
        "{output}"
    );
    assert!(output.contains("out"), "{output}");
    assert!(output.contains("err"), "{output}");
    assert!(output.contains("sent to shell"), "{output}");
    assert!(!output.contains("Tool result for request req-1"));
    assert_inline_before_followup(&output, "$ printf 'out\\n'; printf 'err\\n' >&2", "out");
    assert!(!output.contains("stderr captured; /details"), "{output}");
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approved_sudo_tool_is_emitted_to_foreground_shell() {
    let home = temp_shell_home("approval-sudo-shell");
    write_cosh_config(
        &home,
        r#"[shell]
readonly_disabled = ["git status", "pwd"]"#,
    );
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_sudo = bin_dir.join("sudo");
    write_executable(
        &fake_sudo,
        "#!/bin/sh\nprintf 'fake-sudo:'\n\"$@\"\nprintf '\\n'\n",
    );

    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"?? stream sudo tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"exit\n".to_vec(), Duration::from_millis(2_000)),
        ],
    );

    assert_approval_prompt_visible(&output);
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("$ sudo printf approved-sudo"), "{output}");
    assert!(output.contains("fake-sudo:approved-sudo"), "{output}");
    assert!(
        !output.contains("Tool result for request req-1"),
        "{output}"
    );
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approved_ssh_tool_receives_foreground_input() {
    let home = temp_shell_home("approval-fake-ssh");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("ssh"),
        "#!/bin/sh\nprintf 'fake-ssh prompt:'\nIFS= read -r line\nprintf 'fake-ssh received:%s\\n' \"$line\"\n",
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"?? stream ssh tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"hello-from-user\n".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ ssh fake-host"), "{output}");
    assert!(output.contains("fake-ssh prompt:"), "{output}");
    assert!(
        output.contains("fake-ssh received:hello-from-user"),
        "{output}"
    );
    assert!(
        !output.contains("Tool result for request req-1"),
        "{output}"
    );
}

#[test]
fn raw_cli_approved_pager_tool_receives_q() {
    let home = temp_shell_home("approval-fake-pager");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("fake-pager"),
        "#!/bin/bash\nprintf 'fake-pager waiting\\n'\nIFS= read -r -n 1 key\nprintf 'fake-pager key:%s\\n' \"$key\"\n",
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"?? stream pager tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"q".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("$ fake-pager"), "{output}");
    assert!(output.contains("fake-pager waiting"), "{output}");
    assert!(output.contains("fake-pager key:q"), "{output}");
    assert!(
        !output.contains("Tool result for request req-1"),
        "{output}"
    );
}

#[test]
fn raw_cli_approved_repl_tool_receives_followup_lines() {
    let home = temp_shell_home("approval-fake-repl");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("fake-repl"),
        "#!/bin/sh\nprintf 'fake-repl ready\\n'\nIFS= read -r first\nIFS= read -r second\nprintf 'fake-repl lines:%s/%s\\n' \"$first\" \"$second\"\n",
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"?? stream repl tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (
                b"plain natural language for repl\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b".exit\n".to_vec(), Duration::from_millis(300)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("$ fake-repl"), "{output}");
    assert!(output.contains("fake-repl ready"), "{output}");
    assert!(
        output.contains("fake-repl lines:plain natural language for repl/.exit"),
        "{output}"
    );
    assert!(!output.contains("AI request"), "{output}");
    assert!(
        !output.contains("Tool result for request req-1"),
        "{output}"
    );
}

#[test]
fn raw_cli_approved_bash_tool_drops_stale_pre_approval_followup() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream stale tool approval\n".to_vec(), Duration::ZERO),
        (b"\n".to_vec(), Duration::from_millis(1_400)),
        (b"exit\n".to_vec(), Duration::from_millis(500)),
    ]);
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert!(output.contains("Preparing a command before approval."));
    assert_approval_prompt_visible(&output);
    assert!(output.contains("req-1"), "{output}");
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("sent to shell"), "{output}");
    assert!(output.contains("$ pwd"), "{output}");
    assert!(output.contains(expected_cwd), "{output}");
    assert!(!output.contains("Tool result for request req-1"));
    assert!(
        !output.contains("STALE APPROVAL TEXT SHOULD NOT RENDER"),
        "{output}"
    );
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_denied_bash_tool_does_not_render_stale_executed_claim() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream pwd tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1b[C\x1b[C\n".to_vec(), Duration::from_millis(800)),
        (b"exit\n".to_vec(), Duration::from_millis(300)),
    ]);
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert!(output.contains("Preparing a streamed pwd request before finishing."));
    assert_approval_prompt_visible(&output);
    assert!(output.contains("Subject: Bash"), "{output}");
    assert!(output.contains("Denied req-1"), "{output}");
    assert!(!output.contains("No command ran."), "{output}");
    assert!(
        output.contains("Command was not executed for req-1"),
        "{output}"
    );
    assert!(!output.contains(expected_cwd), "{output}");
    assert!(
        !output.contains("approved Bash command finished"),
        "{output}"
    );
    assert!(
        !output.contains("Command result analysis for req-1"),
        "{output}"
    );
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_denied_bash_tool_uses_zh_language_env() {
    let output = run_raw_cli_ask_with_args_env_and_delayed_input(
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"?? stream pwd tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b[C\x1b[C\n".to_vec(), Duration::from_millis(2_500)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert_zh_approval_prompt_visible(&output);
    assert!(output.contains("对象: Bash"), "{output}");
    assert!(output.contains("已拒绝 req-1"), "{output}");
    assert!(output.contains("$ pwd"), "{output}");
    assert!(
        output.contains("Command was not executed for req-1"),
        "{output}"
    );
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("Approval req-1"), "{output}");
    assert!(!output.contains("Subject: Bash"), "{output}");
    assert!(!output.contains("Denied req-1"), "{output}");
    assert!(!output.contains(expected_cwd), "{output}");
    assert!(
        !output.contains("approved Bash command finished"),
        "{output}"
    );
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_user_approved_bash_tool_supports_pipe() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream piped tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"exit\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );

    assert!(output.contains("Preparing a piped streamed tool request before finishing."));
    assert_approval_prompt_visible(&output);
    assert!(output.contains("Subject: Bash"));
    assert!(output.contains("$ ps aux | head"));
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Blocked req-1"), "{output}");
    assert!(output.contains("$ ps aux | head"), "{output}");
    assert!(
        !output.contains("cosh-shell: blocked shell metacharacter"),
        "{output}"
    );
    assert!(output.contains("sent to shell"), "{output}");
    assert!(!output.contains("approved Bash command finished"));
    assert!(!output.contains("Tool result for request req-1"));
    assert!(!output.contains("Received approved tool result"));
    assert!(!output.contains("Analysis continued after the approved command"));
    assert!(!output.contains("Thinking...Approval"));
}
