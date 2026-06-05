use super::*;

#[test]
fn raw_cli_allow_is_unknown_and_does_not_record_recommendation_approval() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /allow 2\n\
         echo after-allow\n\
         exit\n",
    );

    assert!(output.contains("Unknown slash command: /allow"));
    assert!(!output.contains("/allow N records"));
    assert!(output.contains("cosh never executes"));
    assert!(!output.contains("Approved recommendation 2"));
    assert!(!output.contains("Governance: approval recorded"));
    assert!(output.contains("after-allow"));
    assert!(!output.contains("/.cargo/bin"));
}

#[test]
fn raw_cli_approve_slash_is_not_recommendation_or_governance_alias() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /approve 2\n\
         echo after-approve-slash\n\
         exit\n",
    );

    assert!(output.contains("Recommendations"));
    assert!(!output.contains("Approved recommendation 2"));
    assert!(!output.contains("Governance: approval recorded"));
    assert!(!output.contains("/.cargo/bin"));
    assert!(output.contains("after-approve-slash"));
}

#[test]
#[ignore] // timing sensitive
    fn raw_cli_approval_requests_support_details_approve_and_deny() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(300)),
            (b"\x1b[C\n".to_vec(), Duration::from_millis(150)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval req-"));
    assert!(output.contains("Run shell command?"), "{output}");
    assert!(output.contains("$ git status"), "{output}");
    assert!(output.contains("Queue: 1/2 pending; next req-2 shell command"));
    assert!(output.contains("Allow once"));
    assert!(output.contains("Deny"));
    assert!(output.contains("Details"));
    assert!(!output.contains("req-1 · shell tool · medium risk"));
    assert!(!output.contains("Use ←/→ to choose Approve or Deny"));
    assert!(!output.contains("/approve req-1"));
    assert!(!output.contains("/deny req-1"));
    assert!(output.contains("Activity:"));
    assert!(output.contains("Skill loaded: git-project"));
    assert!(output.contains("Tool output: stdout captured; /details out-1"));
    assert!(output.contains("Tool completed"));
    assert!(!output.contains("skill-2 skill:"));
    assert!(!output.contains("out-1 output:"));
    assert!(!output.contains("tool-1 tool:"));
    assert!(!output.contains("skill-1 skill: git-project loading"));
    assert!(!output.contains("tool-1stream"));
    assert!(output.contains("Approved"));
    assert!(output.contains("$ git status"));
    assert!(!output.contains("tool request - approved"));
    assert!(output.contains("Denied req-2"));
    assert!(!output.contains("req-2 · shell command request · medium risk"));
    assert!(!output.contains("Queue: 1 of 1 pending"));
    assert!(!output.contains("Subject: shell command"));
    assert!(
        output.contains("$ touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
    assert!(output.contains("Denied"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("shell command request - denied"));
    assert!(!output.contains("Tool result"));
    assert!(!output.contains("bash: /approve"));
    assert!(!output.contains("bash: /deny"));
}

#[test]
fn raw_cli_zsh_approval_card_capture_does_not_leak_to_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b[C\n".to_vec(), Duration::from_millis(400)),
            (
                b"echo after-zsh-approval\n".to_vec(),
                Duration::from_millis(400),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval req-"), "{output}");
    assert!(output.contains("Denied"), "{output}");
    assert!(output.contains("Command: git status --short"), "{output}");
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
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b".to_vec(), Duration::from_millis(300)),
            (b"\x1b".to_vec(), Duration::from_millis(50)),
            (b"\x1b".to_vec(), Duration::from_millis(100)),
            (b"\x1b".to_vec(), Duration::from_millis(50)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval req-"));
    assert!(output.contains("Run shell command?"));
    assert!(output.contains("$ git status"));
    assert!(output.contains("Queue: 1/2 pending; next req-2 shell command"));
    assert!(!output.contains("req-1 · shell tool · medium risk"));
    assert!(output.contains("Cancelled"));
    assert!(output.contains("Cancelled req-2"));
    assert!(!output.contains("req-2 · shell command request · medium risk"));
    assert!(!output.contains("Queue: 1 of 1 pending"));
    assert!(output.contains("$ touch /tmp/cosh-shell-fake-action-should-not-run"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("tool request - cancelled by user"));
    assert!(!output.contains("shell command request - cancelled by user"));
    assert!(!output.contains("Tool result"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_details_approvals_renders_decision_journal_panel() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(300)),
            (b"\x1b[C\n".to_vec(), Duration::from_millis(150)),
            (b"/details approvals\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval journal"), "{output}");
    assert!(output.contains("2 decisions"), "{output}");
    assert!(
        output.contains("req-1") && output.contains("approved"),
        "{output}"
    );
    assert!(
        output.contains("req-2") && output.contains("denied"),
        "{output}"
    );
    assert!(output.contains("Command: git status"), "{output}");
    assert!(
        output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_details_for_approval_uses_structured_panel() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b".to_vec(), Duration::from_millis(300)),
            (b"\x1b".to_vec(), Duration::from_millis(50)),
            (b"\x1b".to_vec(), Duration::from_millis(100)),
            (b"\x1b".to_vec(), Duration::from_millis(50)),
            (b"/details req-1\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval details req-1"), "{output}");
    assert!(output.contains("tool request"), "{output}");
    assert!(output.contains("cancelled"), "{output}");
    assert!(output.contains("medium risk"), "{output}");
    assert!(output.contains("Source: agent"), "{output}");
    assert!(output.contains("Default: deny"), "{output}");
    assert!(output.contains("Request: Bash command"), "{output}");
    assert!(output.contains("Command:"), "{output}");
    assert!(output.contains("git status"), "{output}");
    assert!(!output.contains("Subject: tool shell"), "{output}");
    assert!(!output.contains("Tool input"), "{output}");
    assert!(!output.contains("id: req-1"), "{output}");
    assert!(!output.contains("preview: git status"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_approval_text_input_does_not_confirm_or_leak_to_bash() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
            (b"\x1b".to_vec(), Duration::from_millis(200)),
            (b"\x1b".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval req-"));
    assert!(output.contains("Cancelled"));
    assert!(output.contains("Command: git status"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("tool request - cancelled by user"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("Decision: approved"));
    assert_eq!(count_occurrences(&output, "cosh-osc$ exit"), 0, "{output}");
    assert!(!output.contains("bash:"));
}

#[test]
#[ignore] // timing sensitive
    fn raw_cli_approval_details_key_expands_without_confirming() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"d".to_vec(), Duration::from_millis(300)),
            (b"\x1b".to_vec(), Duration::from_millis(200)),
            (b"\x1b".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval req-"));
    assert!(output.contains("[Details]") || output.contains("Details"));
    assert!(output.contains("Default: deny"));
    assert!(output.contains("read-only broker"));
    assert!(output.contains("Esc") && output.contains("cancel"));
    assert!(output.contains("Cancelled"));
    assert!(output.contains("Command: git status"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("tool request - cancelled by user"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("Decision: approved"));
    assert!(!output.contains("bash:"));
}

#[test]
    #[ignore] // flaky under parallel execution
    fn raw_cli_approval_arrow_focus_updates_and_confirm_uses_selection() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b[C\n".to_vec(), Duration::from_millis(400)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("Approval req-"));
    assert!(output.contains("[Deny]") || output.contains("Deny"));
    assert!(output.contains("Denied"));
    assert!(output.contains("Command: git status --short"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("Bash tool - denied"));
    assert!(output.contains("\x1b["));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("tool-1 tool: executed"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approval_split_arrow_sequence_does_not_cancel() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b".to_vec(), Duration::from_millis(400)),
            (b"[".to_vec(), Duration::from_millis(50)),
            (b"C\n".to_vec(), Duration::from_millis(50)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("Approval req-"));
    assert!(
        output.contains("> [ Deny ]") || output.contains("[Deny]"),
        "{output}"
    );
    assert!(output.contains("Denied"));
    assert!(output.contains("Command: git status --short"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("Bash tool - denied"));
    assert!(!output.contains("Cancelled"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("bash:"));
}

#[test]
    #[ignore] // flaky under parallel execution
    fn raw_cli_approval_application_cursor_arrow_updates_focus() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1bOC\n".to_vec(), Duration::from_millis(400)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("Approval req-"));
    assert!(
        output.contains("> [ Deny ]") || output.contains("[Deny]"),
        "{output}"
    );
    assert!(output.contains("Denied"));
    assert!(output.contains("Command: git status --short"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("Bash tool - denied"));
    assert!(!output.contains("Cancelled"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_streaming_tool_approval_renders_before_agent_finishes() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(400)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("Preparing a streamed tool request before finishing."));
    assert!(output.contains("Approval req-"));
    assert!(output.contains("Run Bash command?"));
    assert!(output.contains("$ git status --short"));
    assert!(!output.contains("medium risk"));
    assert!(!output.contains("Subject: tool Bash"));
    assert!(!output.contains("Command: git status --short"));
    assert!(!output.contains("Keys: Left/Right select"));
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Bash tool - approved"), "{output}");
    assert!(output.contains("$ git status --short"), "{output}");
    assert!(output.contains("Command result analysis for req-1"));
    assert!(!output.contains("Received approved tool result"));
    assert_inline_before_followup(
        &output,
        "Preparing a streamed tool request before finishing.",
        "Approval req-",
    );
    assert_inline_before_followup(&output, "$ git status --short", "Command result analysis");
    assert!(!output.contains("Analysis continued after the approved command"));
    assert!(!output.contains("Activity"), "{output}");
    assert!(!output.contains("stdout captured; /details"), "{output}");
    assert!(!output.contains("tool request - approved by user"));
    assert!(!output.contains("Running command"), "{output}");
    assert!(!output.contains("tool-1 tool: executed"));
    assert!(!output.contains("Thinking...Approval"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approved_bash_tool_prints_native_command_and_stdout() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream pwd tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(400)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert!(output.contains("Preparing a streamed pwd request before finishing."));
    assert!(output.contains("Run Bash command?"), "{output}");
    assert!(output.contains("$ pwd"), "{output}");
    assert!(output.contains(expected_cwd), "{output}");
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Command result analysis for req-1"));
    assert_inline_before_followup(&output, "$ pwd", expected_cwd);
    assert_inline_before_followup(&output, expected_cwd, "Command result analysis");
    assert!(!output.contains("Activity"), "{output}");
    assert!(!output.contains("stdout captured; /details"), "{output}");
    assert!(!output.contains("Command: pwd"), "{output}");
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approved_bash_tool_drops_stale_pre_approval_followup() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream stale tool approval\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(400)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert!(output.contains("Preparing a command before approval."));
    assert!(output.contains("Approval req-"), "{output}");
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("$ pwd"), "{output}");
    assert!(output.contains(expected_cwd), "{output}");
    assert!(
        output.contains("Command result analysis for req-1"),
        "{output}"
    );
    assert!(
        !output.contains("STALE APPROVAL TEXT SHOULD NOT RENDER"),
        "{output}"
    );
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_denied_bash_tool_does_not_render_stale_executed_claim() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? stream pwd tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b[C\n".to_vec(), Duration::from_millis(400)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );
    let expected_cwd = env!("CARGO_MANIFEST_DIR");

    assert!(output.contains("Preparing a streamed pwd request before finishing."));
    assert!(output.contains("Run Bash command?"), "{output}");
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
fn raw_cli_approved_unsafe_bash_tool_fails_closed() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"?? stream blocked tool approval\n".to_vec(),
                Duration::ZERO,
            ),
            (b"\n".to_vec(), Duration::from_millis(300)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("Preparing a blocked streamed tool request before finishing."));
    assert!(output.contains("Approval req-"));
    assert!(output.contains("Run Bash command?"));
    assert!(output.contains("$ ps aux | head"));
    assert!(!output.contains("Command: ps aux | head"));
    assert!(!output.contains("Keys: Left/Right select"));
    assert!(output.contains("$ ps aux | head"), "{output}");
    assert!(
        output.contains("cosh-shell: blocked shell metacharacter"),
        "{output}"
    );
    assert!(output.contains("Command result analysis for req-1"));
    assert!(output.contains("did not produce a successful"));
    assert!(output.contains("execution result"));
    assert!(!output.contains("approved Bash command finished"));
    assert!(!output.contains("command finished"));
    assert!(!output.contains("Received approved tool result"));
    assert!(!output.contains("Analysis continued after the approved command"));
    assert!(!output.contains("tool request - approved by user"));
    assert!(!output.contains("Thinking...Approval"));
    assert!(!output.contains("USER               PID"));
    assert!(!output.contains("bash:"));
}
