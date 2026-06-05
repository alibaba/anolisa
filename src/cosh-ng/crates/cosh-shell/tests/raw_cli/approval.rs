use super::*;

fn run_raw_cli_ask_with_delayed_input(chunks: Vec<(Vec<u8>, Duration)>) -> String {
    run_raw_cli_ask_with_args_and_delayed_input(&[], chunks)
}

fn run_raw_cli_ask_with_args_and_delayed_input(
    args: &[&str],
    chunks: Vec<(Vec<u8>, Duration)>,
) -> String {
    run_raw_cli_ask_with_args_env_and_delayed_input(args, &[], chunks)
}

fn run_raw_cli_ask_with_args_env_and_delayed_input(
    args: &[&str],
    extra_env: &[(&str, &str)],
    chunks: Vec<(Vec<u8>, Duration)>,
) -> String {
    let home = temp_shell_home("approval-cards");
    write_cosh_config(
        &home,
        r#"approval.readonly_disabled = ["git status", "pwd"]"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let mut env = vec![("HOME", home_str.as_str())];
    env.extend_from_slice(extra_env);
    run_raw_cli_with_args_env_and_delayed_input("fake", args, &env, chunks)
}

#[test]
fn raw_cli_allow_is_removed_and_does_not_record_recommendation_approval() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /allow 2\n\
         echo after-allow\n\
         exit\n",
    );

    assert!(!output.contains("Unknown slash command: /allow"));
    assert!(!output.contains("Use /help to see available commands."));
    assert!(output.contains("Command removed"), "{output}");
    assert!(
        output.contains("/allow is no longer a supported input command."),
        "{output}"
    );
    assert!(
        output.contains("Use the approval card buttons instead; nothing was sent to the shell."),
        "{output}"
    );
    assert!(!output.contains("/allow N records"));
    assert!(!output.contains("Approved recommendation 2"));
    assert!(!output.contains("Governance: approval recorded"));
    assert!(output.contains("after-allow"));
    assert!(!output.contains("/.cargo/bin"));
    assert!(!output.contains("bash: /allow"));
}

#[test]
fn raw_cli_approve_slash_is_not_recommendation_or_governance_alias() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /approve 2\n\
         /deny 2\n\
         echo after-approve-slash\n\
         exit\n",
    );

    assert!(output.contains("Recommendations"));
    assert!(!output.contains("Approved recommendation 2"));
    assert!(!output.contains("Governance: approval recorded"));
    assert!(!output.contains("/.cargo/bin"));
    assert!(output.contains("after-approve-slash"));
    assert!(
        output.contains("/approve is no longer a supported input command."),
        "{output}"
    );
    assert!(
        output.contains("/deny is no longer a supported input command."),
        "{output}"
    );
    assert!(
        output.contains("Use the approval card buttons instead; nothing was sent to the shell."),
        "{output}"
    );
    assert!(!output.contains("bash: /approve"), "{output}");
    assert!(!output.contains("bash: /deny"), "{output}");
}

#[test]
fn raw_cli_zsh_approval_card_capture_does_not_leak_to_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_ask_with_args_and_delayed_input(
        &["--shell", "zsh"],
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b[C\x1b[C\n".to_vec(), Duration::from_millis(400)),
            (
                b"echo after-zsh-approval\n".to_vec(),
                Duration::from_millis(400),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(
        output.contains("Approval required") || output.contains("Approval req-1"),
        "{output}"
    );
    assert!(output.contains("Denied"), "{output}");
    assert!(output.contains("$ git status --short"), "{output}");
    assert!(!output.contains("No command ran."), "{output}");
    assert!(output.contains("after-zsh-approval"), "{output}");
    assert!(!output.contains("zsh: command not found"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory"),
        "{output}"
    );
    assert!(!output.contains("^[[C"), "{output}");
    assert!(!output.contains("\x1b]1337;COSH;"), "{output}");
}

#[test]
fn raw_cli_approval_cancel_records_receipt_and_advances_queue() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? request tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1b\n".to_vec(), Duration::from_millis(2_500)),
        (b"\x1b\n".to_vec(), Duration::from_millis(500)),
        (b"exit\n".to_vec(), Duration::from_millis(200)),
    ]);

    assert!(output.contains("Approval required"));
    assert!(output.contains("req-1 · tool request · medium risk"));
    assert!(output.contains("$ git status"));
    assert!(output.contains("Queue: 1 of 1 pending"));
    assert!(!output.contains("req-1 · shell tool · medium risk"));
    assert!(output.contains("Cancelled"), "{output}");
    assert!(!output.contains("Cancelled req-2"));
    assert!(!output.contains("req-2 · shell command request · medium risk"));
    assert!(!output.contains("Queue: 1 of 2 pending"));
    assert!(!output.contains("$ touch /tmp/cosh-shell-fake-action-should-not-run"));
    assert!(!output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("tool request - cancelled by user"));
    assert!(!output.contains("shell command request - cancelled by user"));
    assert!(!output.contains("Tool result"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approval_ctrl_c_cancels_card_without_agent_cancel() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? request tool approval\n".to_vec(), Duration::ZERO),
        (vec![0x03], Duration::from_millis(2_500)),
        (
            b"echo after-approval-ctrl-c\n".to_vec(),
            Duration::from_millis(500),
        ),
        (b"exit\n".to_vec(), Duration::from_millis(200)),
    ]);

    assert!(output.contains("Approval required"), "{output}");
    assert!(output.contains("Cancelled req-1"), "{output}");
    assert!(output.contains("after-approval-ctrl-c"), "{output}");
    assert!(!output.contains("Agent cancellation requested"), "{output}");
    assert!(
        !output.contains("Reason: user requested cancellation"),
        "{output}"
    );
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approval_card_uses_zh_language_env() {
    let output = run_raw_cli_ask_with_args_env_and_delayed_input(
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(2_500)),
            (b"exit\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );

    assert!(output.contains("需要审批"), "{output}");
    assert!(output.contains("对象: Bash"), "{output}");
    assert!(
        output.contains("Tool 输入: $ git status --short"),
        "{output}"
    );
    assert!(output.contains("允许一次"), "{output}");
    assert!(output.contains("始终信任"), "{output}");
    assert!(output.contains("拒绝"), "{output}");
    assert!(output.contains("已批准 req-1"), "{output}");
    assert!(output.contains("已发送到 shell"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("Subject: Bash"), "{output}");
    assert!(!output.contains("Allow once"), "{output}");
    assert!(!output.contains("Always trust"), "{output}");
    assert!(!output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("sent to shell"), "{output}");
    assert!(!output.contains("bash:"), "{output}");
    assert_no_migrated_english_ui_labels(&output, APPROVAL_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_details_approvals_renders_decision_journal_panel() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? request tool approval\n".to_vec(), Duration::ZERO),
        (b"\n".to_vec(), Duration::from_millis(2_500)),
        (
            b"/details approvals\n".to_vec(),
            Duration::from_millis(1_500),
        ),
        (b"exit\n".to_vec(), Duration::from_millis(500)),
    ]);

    assert!(output.contains("Approval journal"), "{output}");
    assert!(output.contains("1 decisions"), "{output}");
    assert!(
        output.contains("req-1") && output.contains("approved"),
        "{output}"
    );
    assert!(
        !(output.contains("req-2") && output.contains("denied")),
        "{output}"
    );
    assert!(output.contains("Command: $ git status"), "{output}");
    assert!(
        output.contains("Execution: foreground_shell_pty"),
        "{output}"
    );
    assert!(output.contains("Command block:"), "{output}");
    assert!(output.contains("Redaction: ref_only"), "{output}");
    assert!(
        !output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_details_approvals_records_denied_not_executed() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1b[C\x1b[C\n".to_vec(), Duration::from_millis(800)),
        (
            b"/details approvals\n".to_vec(),
            Duration::from_millis(1_000),
        ),
        (b"exit\n".to_vec(), Duration::from_millis(500)),
    ]);

    assert!(output.contains("Denied req-1"), "{output}");
    assert!(output.contains("Approval journal"), "{output}");
    assert!(output.contains("1 decisions"), "{output}");
    assert!(
        output.contains("req-1") && output.contains("denied"),
        "{output}"
    );
    assert!(
        output.contains("Execution: not_executed_denied"),
        "{output}"
    );
    assert!(
        !output.contains("Execution: foreground_shell_pty"),
        "{output}"
    );
    assert!(output.contains("Command block: <none>"), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_details_approvals_records_cancelled_not_executed() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1b\n".to_vec(), Duration::from_millis(800)),
        (
            b"/details approvals\n".to_vec(),
            Duration::from_millis(1_000),
        ),
        (b"exit\n".to_vec(), Duration::from_millis(500)),
    ]);

    assert!(output.contains("Cancelled req-1"), "{output}");
    assert!(output.contains("Approval journal"), "{output}");
    assert!(output.contains("1 decisions"), "{output}");
    assert!(
        output.contains("req-1") && output.contains("cancelled"),
        "{output}"
    );
    assert!(
        output.contains("Execution: not_executed_cancelled"),
        "{output}"
    );
    assert!(
        !output.contains("Execution: foreground_shell_pty"),
        "{output}"
    );
    assert!(output.contains("Command block: <none>"), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_details_approvals_uses_zh_language_env() {
    let output = run_raw_cli_ask_with_args_env_and_delayed_input(
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(2_500)),
            (
                b"/details approvals\n".to_vec(),
                Duration::from_millis(1_500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("审批记录"), "{output}");
    assert!(output.contains("1 条决策"), "{output}");
    assert!(
        output.contains("req-1") && output.contains("approved"),
        "{output}"
    );
    assert!(output.contains("命令: $ git status"), "{output}");
    assert!(output.contains("执行: foreground_shell_pty"), "{output}");
    assert!(output.contains("命令块:"), "{output}");
    assert!(output.contains("脱敏: ref_only"), "{output}");
    assert!(!output.contains("Approval journal"), "{output}");
    assert!(!output.contains("Command: $ git status"), "{output}");
    assert!(
        !output.contains("Execution: foreground_shell_pty"),
        "{output}"
    );
    assert!(!output.contains("Command block:"), "{output}");
    assert!(!output.contains("Redaction: ref_only"), "{output}");
    assert!(
        !output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
    assert!(!output.contains("bash: /details"), "{output}");
    assert_no_migrated_english_ui_labels(&output, APPROVAL_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_details_for_approval_uses_structured_panel() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"d".to_vec(), Duration::from_millis(2_500)),
        (b"\x1b".to_vec(), Duration::from_millis(300)),
        (b"exit\n".to_vec(), Duration::from_millis(800)),
    ]);

    assert!(
        output.contains("Approval required") || output.contains("Approval req-1"),
        "{output}"
    );
    assert!(output.contains("tool request"), "{output}");
    assert!(output.contains("Cancelled req-1"), "{output}");
    assert!(output.contains("medium risk"), "{output}");
    assert!(
        output.contains("Policy: user approval is required before any executable tool request"),
        "{output}"
    );
    assert!(
        output.contains("Keys:") && output.contains("d details"),
        "{output}"
    );
    assert!(output.contains("Command:"), "{output}");
    assert!(output.contains("git status"), "{output}");
    assert!(!output.contains("Subject: tool shell"), "{output}");
    assert!(!output.contains("id: req-1"), "{output}");
    assert!(!output.contains("preview: git status"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_details_for_approval_uses_zh_language_env() {
    let output = run_raw_cli_ask_with_args_env_and_delayed_input(
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"d".to_vec(), Duration::from_millis(2_500)),
            (b"\x1b".to_vec(), Duration::from_millis(300)),
            (b"exit\n".to_vec(), Duration::from_millis(800)),
        ],
    );

    assert!(output.contains("需要审批"), "{output}");
    assert!(output.contains("已取消 req-1"), "{output}");
    assert!(output.contains("风险 medium"), "{output}");
    assert!(
        output.contains("策略: 可执行 tool 请求必须先经过用户审批。"),
        "{output}"
    );
    assert!(output.contains("命令:"), "{output}");
    assert!(output.contains("git status"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("Approval details"), "{output}");
    assert!(!output.contains("Cancelled req-1"), "{output}");
    assert!(
        !output.contains("Policy: user approval is required"),
        "{output}"
    );
    assert!(!output.contains("Keys:"), "{output}");
    assert!(!output.contains("Command:"), "{output}");
    assert!(!output.contains("Subject: tool shell"), "{output}");
    assert!(!output.contains("id: req-1"), "{output}");
    assert!(!output.contains("preview: git status"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
    assert_no_migrated_english_ui_labels(&output, APPROVAL_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_multiline_bash_tool_is_visible_in_approval_details() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (
            b"?? stream multiline tool approval\n".to_vec(),
            Duration::ZERO,
        ),
        (b"d".to_vec(), Duration::from_millis(2_500)),
        (b"\x1b".to_vec(), Duration::from_millis(300)),
        (b"exit\n".to_vec(), Duration::from_millis(200)),
    ]);

    assert!(output.contains("Approval required"), "{output}");
    assert!(output.contains("Command:"), "{output}");
    assert!(output.contains("printf one"), "{output}");
    assert!(output.contains("printf two"), "{output}");
    assert!(!output.contains("bash:"), "{output}");
}

#[test]
fn raw_cli_approval_text_input_does_not_confirm_or_leak_to_bash() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"exit\n".to_vec(), Duration::from_millis(5_000)),
        (b"\x1b".to_vec(), Duration::from_millis(200)),
        (b"exit\n".to_vec(), Duration::from_millis(200)),
    ]);

    assert!(output.contains("Approval required"));
    assert!(output.contains("req-1 · tool request · medium risk"));
    assert!(output.contains("Cancelled"));
    assert!(output.contains("$ git status"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("tool request - cancelled by user"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("Decision: approved"));
    assert_eq!(count_occurrences(&output, "cosh-osc$ exit"), 1, "{output}");
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approval_split_arrow_sequence_does_not_cancel() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1b".to_vec(), Duration::from_millis(5_000)),
        (b"[".to_vec(), Duration::from_millis(50)),
        (b"C".to_vec(), Duration::from_millis(50)),
        (b"\x1b[C".to_vec(), Duration::from_millis(50)),
        (b"\n".to_vec(), Duration::from_millis(100)),
        (b"exit\n".to_vec(), Duration::from_millis(1_000)),
    ]);

    assert!(output.contains("Approval required"));
    assert!(output.contains("req-1 · tool request · medium risk"));
    assert!(
        output.contains("> [ Deny ]") || output.contains("[Deny]"),
        "{output}"
    );
    assert!(output.contains("Denied"));
    assert!(output.contains("$ git status --short"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("Bash tool - denied"));
    assert!(!output.contains("Cancelled"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approval_application_cursor_arrow_updates_focus() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1bOC".to_vec(), Duration::from_millis(5_000)),
        (b"\x1bOC".to_vec(), Duration::from_millis(100)),
        (b"\n".to_vec(), Duration::from_millis(100)),
        (b"exit\n".to_vec(), Duration::from_millis(1_000)),
    ]);

    assert!(output.contains("Approval required"));
    assert!(output.contains("req-1 · tool request · medium risk"));
    assert!(
        output.contains("> [ Deny ]") || output.contains("[Deny]"),
        "{output}"
    );
    assert!(output.contains("Denied"));
    assert!(output.contains("$ git status --short"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("Bash tool - denied"));
    assert!(!output.contains("Cancelled"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_streaming_tool_approval_renders_before_agent_finishes() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\n".to_vec(), Duration::from_millis(2_500)),
        (b"exit\n".to_vec(), Duration::from_millis(1_000)),
    ]);

    assert!(output.contains("Preparing a streamed tool request before finishing."));
    assert!(output.contains("Approval required"));
    assert!(output.contains("Subject: Bash"));
    assert!(output.contains("$ git status --short"));
    assert!(output.contains("medium risk"));
    assert!(!output.contains("Subject: tool Bash"));
    assert!(!output.contains("Command: git status --short"));
    assert!(!output.contains("Keys: Left/Right select"));
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("sent to shell"), "{output}");
    assert!(!output.contains("Bash tool - approved"), "{output}");
    assert!(output.contains("$ git status --short"), "{output}");
    assert!(!output.contains("Tool result for request req-1"));
    assert!(!output.contains("Received approved tool result"));
    assert_inline_before_followup(
        &output,
        "Preparing a streamed tool request before finishing.",
        "Approval required",
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
    assert!(output.contains("Approval required"), "{output}");
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
    assert!(output.contains("Approval required"), "{output}");
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
    assert!(output.contains("Approval required"), "{output}");
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
        r#"approval.readonly_disabled = ["git status", "pwd"]"#,
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

    assert!(output.contains("Approval required"), "{output}");
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
    assert!(output.contains("Approval required"), "{output}");
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
    assert!(output.contains("Approval required"), "{output}");
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
            (b"\x1b[C\x1b[C\n".to_vec(), Duration::from_millis(800)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert!(output.contains("需要审批"), "{output}");
    assert!(output.contains("对象: Bash"), "{output}");
    assert!(output.contains("已拒绝 req-1"), "{output}");
    assert!(output.contains("$ pwd"), "{output}");
    assert!(
        output.contains("Command was not executed for req-1"),
        "{output}"
    );
    assert!(!output.contains("Approval required"), "{output}");
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
    assert!(output.contains("Approval required"));
    assert!(output.contains("Subject: Bash"));
    assert!(output.contains("$ ps aux | head"));
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Blocked req-1"), "{output}");
    assert!(!output.contains("Keys: Left/Right select"));
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
