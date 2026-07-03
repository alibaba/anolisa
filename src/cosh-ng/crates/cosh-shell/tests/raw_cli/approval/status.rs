use super::*;

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

    assert_approval_request_card_visible(&output);
    assert!(output.contains("req-1"));
    assert!(output.contains("tool request"));
    assert!(output.contains("medium risk"));
    assert!(output.contains("$ git status"));
    assert!(output.contains("Queue: 1 of 1 pending") || output.contains("Queue: 1/1 pending"));
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

    assert_approval_request_card_visible(&output);
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

    assert_approval_request_card_visible(&output);
    assert!(output.contains("对象: Bash"), "{output}");
    assert!(output.contains("Tool 输入:"), "{output}");
    assert!(output.contains("$ git status --short"), "{output}");
    assert!(output.contains("允许一次"), "{output}");
    assert!(output.contains("始终信任"), "{output}");
    assert!(output.contains("拒绝"), "{output}");
    assert!(output.contains("已批准 req-1"), "{output}");
    assert!(output.contains("已发送到 shell"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("Approval req-"), "{output}");
    assert!(!output.contains("Subject: Bash"), "{output}");
    assert!(!output.contains("Allow once"), "{output}");
    assert!(!output.contains("Always trust"), "{output}");
    assert!(!output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("sent to shell"), "{output}");
    assert!(!output.contains("bash:"), "{output}");
    assert_no_migrated_english_ui_labels(&output, APPROVAL_ZH_FORBIDDEN_UI);
}
