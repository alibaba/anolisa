use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[path = "raw_cli/approval.rs"]
mod approval;
#[path = "raw_cli/question.rs"]
mod question;

#[test]
fn raw_cli_inline_guidance_works_with_fake_adapter() {
    let output = run_raw_cli("fake");

    assert!(output.contains("Thinking..."));
    assert!(!output.contains("Agent status"));
    assert!(!output.contains("Phase: analyzing"));
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert_inline_before_followup(&output, "The command", "after-inline");
}

#[test]
fn raw_cli_startup_banner_renders_when_enabled() {
    let output = run_raw_cli_with_env(
        "fake",
        "exit\n",
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(output.contains("cosh-shell"), "{output}");
    assert!(output.contains("┌─┐┌─┐┌─┐┬ ┬"), "logo missing: {output}");
    assert!(output.contains("shell"), "{output}");
    assert!(output.contains("Adapter: fake"), "{output}");
    assert!(output.contains("Shell: bash"), "{output}");
    assert!(output.contains("Mode: ask"), "{output}");
    assert!(output.contains("/help"), "{output}");
    assert!(output.contains("/details"), "{output}");
    assert!(output.contains("/skill"), "{output}");
    assert!(
        output.contains("Startup hooks: none configured"),
        "{output}"
    );
    assert!(!output.contains("no command ran"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ cosh-shell"), "{output}");
    assert_inline_before_followup(&output, "╭ cosh-shell", "cosh-osc$ exit");
    assert!(!output.contains("Thinking..."), "{output}");
}

#[test]
fn raw_cli_startup_hooks_render_markdown_findings_without_running_commands() {
    let output = run_raw_cli_with_env(
        "fake",
        "exit\n",
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("COSH_SHELL_STARTUP_HOOKS", "1"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(
        output.contains("Startup hooks: built-in read-only checks completed"),
        "{output}"
    );
    assert!(output.contains("Startup findings"), "{output}");
    assert!(
        output.contains("Rust project detected from Cargo.toml")
            || output.contains("No startup findings from built-in read-only checks"),
        "{output}"
    );
    assert!(
        output.contains("cosh-shell only inspected lightweight startup context"),
        "{output}"
    );
    assert!(output.contains("Read-only startup checks."), "{output}");
    assert!(!output.contains("No command ran."), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("bash:"), "{output}");
}

#[test]
fn raw_cli_startup_banner_reports_selected_zsh_shell() {
    let output = run_raw_cli_with_args_and_env(
        "fake",
        &["--shell", "zsh"],
        "exit\n",
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(output.contains("cosh-shell"), "{output}");
    assert!(output.contains("Shell: zsh"), "{output}");
    assert!(!output.contains("Shell: bash"), "{output}");
    assert!(!output.contains("zsh: command not found"), "{output}");
}

#[test]
fn raw_cli_inline_guidance_works_with_qwen_adapter() {
    let output = run_raw_cli("qwen");

    assert!(output.contains("Thinking..."));
    assert!(output.contains("Qwen CLI adapter prepared a safe recommend-only invocation"));
    assert!(output.contains("qwen --approval-mode plan <prompt>"));
    assert_inline_before_followup(&output, "Qwen CLI adapter", "after-inline");
}

#[test]
fn raw_cli_shell_arg_can_select_zsh_raw_host() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_and_env(
        "fake",
        &["--shell", "zsh"],
        "echo zsh-cli:$ZSH_VERSION\nexit\n",
        &[("SHELL", "/bin/bash"), ("TERM", "xterm-256color")],
    );

    assert!(output.contains("zsh-cli:5"), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("\x1b]1337;COSH;"), "{output}");
}

#[test]
fn raw_cli_unsupported_shell_reports_error_without_starting_bash() {
    assert_raw_cli_rejects_shell_args(
        &["raw", "fake", "--shell", "fish"],
        "unsupported raw shell: fish; supported shells: bash, zsh",
    );
}

#[test]
fn raw_cli_missing_shell_arg_reports_error_without_starting_bash() {
    assert_raw_cli_rejects_shell_args(
        &["raw", "fake", "--shell"],
        "missing value for --shell; supported shells: bash, zsh",
    );
}

fn assert_raw_cli_rejects_shell_args(args: &[&str], expected: &str) {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run shell selection error case");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stderr.contains(expected),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("cosh-osc$"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stderr.contains("bash:"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_zsh_shell_arg_intercepts_fragmented_agent_marker() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
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

    assert!(output.contains("Thinking..."), "{output}");
    assert!(
        output.contains("Received shell prompt request: ?? zsh fragmented agent"),
        "{output}"
    );
    assert!(output.contains("after-zsh-agent"), "{output}");
    assert!(!output.contains("zsh: command not found: ??"), "{output}");
    assert!(!output.contains("\x1b]1337;COSH;"), "{output}");
}

#[test]
fn raw_cli_zsh_shell_arg_intercepts_fragmented_slash() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        vec![
            (b"/he".to_vec(), Duration::ZERO),
            (b"lp\n".to_vec(), Duration::from_millis(50)),
            (
                b"echo after-zsh-slash\n".to_vec(),
                Duration::from_millis(100),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("Slash commands"), "{output}");
    assert!(output.contains("/mode [ask|auto]"), "{output}");
    assert!(output.contains("after-zsh-slash"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory: /help"),
        "{output}"
    );
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
    assert!(output.contains("Thinking..."), "{output}");
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
fn raw_cli_slash_after_failed_command_invokes_adapter() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n/explain last error\necho after-explain\nexit\n",
    );

    assert!(output.contains("Thinking..."));
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed"));
    assert_inline_before_followup(&output, "Thinking...", "The command");
    assert_inline_before_followup(&output, "The command", "after-explain");
}

#[test]
fn raw_cli_selects_recommendation_without_executing_it() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /explain last error\n\
         /select 2\n\
         echo after-select\n\
         exit\n",
    );

    assert!(output.contains("Recommendations"));
    assert!(output.contains("  1. pwd"));
    assert!(output.contains("  2. echo $PATH"));
    assert!(output.contains("Selected recommendation 2"));
    assert!(output.contains("echo $PATH"));
    assert!(output.contains("Display-only: command was not executed; copy or re-enter it to run"));
    assert!(output.contains("after-select"));
    assert!(!output.contains("/.cargo/bin"));
}

#[test]
fn raw_cli_copy_alias_selects_recommendation_without_executing_it() {
    let output = run_raw_cli_with_input(
        "fake",
        "/explain last error\n\
         ls /path/that/does/not/exist\n\
         /copy 1\n\
         echo after-copy\n\
         exit\n",
    );

    assert!(output.contains("Selected recommendation 1"));
    assert!(output.contains("pwd"));
    assert!(output.contains("Display-only: command was not executed; copy or re-enter it to run"));
    assert!(output.contains("after-copy"));
}

#[test]
fn raw_cli_details_for_activity_uses_structured_panel() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"\x1b".to_vec(), Duration::from_millis(200)),
            (b"\x1b".to_vec(), Duration::from_millis(50)),
            (b"\x1b".to_vec(), Duration::from_millis(200)),
            (b"\x1b".to_vec(), Duration::from_millis(50)),
            (b"/details out-1\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Activity details out-1"), "{output}");
    assert!(output.contains("output - stdout captured"), "{output}");
    assert!(output.contains("Run: fake-run-input-2"), "{output}");
    assert!(output.contains("Detail:"), "{output}");
    assert!(output.contains("tool: tool-1"), "{output}");
    assert!(output.contains("stream: stdout"), "{output}");
    assert!(output.contains("line 24: fake tool output"), "{output}");
    assert!(output.contains("Skill loaded: git-project"), "{output}");
    assert!(
        output.contains("Tool output: stdout captured; /details out-1"),
        "{output}"
    );
    assert!(output.contains("Tool completed"), "{output}");
    assert!(!output.contains("skill-2 skill:"), "{output}");
    assert!(!output.contains("out-1 output:"), "{output}");
    assert!(!output.contains("tool-1 tool:"), "{output}");
    assert!(!output.contains("id: out-1"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_select_before_recommendation_is_display_only_noop() {
    let output = run_raw_cli_with_input("fake", "/select 1\necho after-early-select\nexit\n");

    assert!(output.contains("No selectable recommendation is available yet"));
    assert!(output.contains("after-early-select"));
    assert!(!output.contains("The command ls "));
}

#[test]
fn raw_cli_select_out_of_range_uses_structured_notice() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /explain last error\n\
         /select 99\n\
         echo after-missing-select\n\
         exit\n",
    );

    assert!(output.contains("Recommendation unavailable"), "{output}");
    assert!(
        output.contains("Recommendation 99 is not available; choose 1..3"),
        "{output}"
    );
    assert!(output.contains("after-missing-select"), "{output}");
    assert!(!output.contains("bash: /select"), "{output}");
}

#[test]
fn raw_cli_missing_details_uses_structured_notice_and_keeps_shell_usable() {
    let output = run_raw_cli_with_input(
        "fake",
        "/details missing-id\n\
         echo after-missing-details\n\
         exit\n",
    );

    assert!(output.contains("Details unavailable"), "{output}");
    assert!(
        output
            .contains("missing-id is not available; use /details with an approval or activity id"),
        "{output}"
    );
    assert!(output.contains("after-missing-details"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_help_renders_slash_command_reference() {
    let output = run_raw_cli_with_input("fake", "/help\necho after-help\nexit\n");

    assert!(output.contains("Slash commands"), "{output}");
    assert!(output.contains("/mode [ask|auto]"), "{output}");
    assert!(output.contains("/details <id>"), "{output}");
    assert!(output.contains("/skill"), "{output}");
    assert!(output.contains("/config"), "{output}");
    assert!(output.contains("/audit"), "{output}");
    assert!(
        output.contains("/approval-mode [ask|auto] - alias for /mode"),
        "{output}"
    );
    assert!(!output.contains("/allow <n>"), "{output}");
    assert!(!output.contains("[ask|auto]alias"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash commands"), "{output}");
    assert!(output.contains("Mode: ask."), "{output}");
    assert!(output.contains("after-help"), "{output}");
    assert!(!output.contains("bash: /help"), "{output}");
}

#[test]
fn raw_cli_informational_slash_commands_render_feedback() {
    let output = run_raw_cli_with_input(
        "fake",
        "/skill\n\
         /config\n\
         /audit\n\
         echo after-info-slash\n\
         exit\n",
    );

    assert!(output.contains("Skill"), "{output}");
    assert!(
        output.contains("No external skill registry is configured"),
        "{output}"
    );
    assert!(output.contains("Config"), "{output}");
    assert!(
        output.contains("Session-local controls: /mode ask|auto"),
        "{output}"
    );
    assert!(output.contains("Audit"), "{output}");
    assert!(
        output.contains("Approval decisions are available with /details approvals"),
        "{output}"
    );
    assert!(output.contains("after-info-slash"), "{output}");
    assert!(!output.contains("bash: /skill"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
    assert!(!output.contains("bash: /audit"), "{output}");
}

#[test]
fn raw_cli_bare_slash_is_noop_without_hint_card() {
    let output = run_raw_cli_with_input(
        "fake",
        "/\n\
         echo after-bare-slash\n\
         exit\n",
    );

    assert!(!output.contains("Slash command hint"), "{output}");
    assert!(!output.contains("/help  /mode"), "{output}");
    assert!(!output.contains("bash: /"), "{output}");
    assert!(
        output.contains("cosh-osc$ echo after-bare-slash"),
        "{output}"
    );
    assert!(output.contains("after-bare-slash"), "{output}");
}

#[test]
fn raw_cli_slash_prefix_renders_hint_and_unknown_feedback() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mo\n\
         /modd\n\
         echo after-slash-hint\n\
         exit\n",
    );

    assert!(output.contains("Slash command hint"), "{output}");
    assert!(
        output.contains("/mode [ask|auto] - show or change approval mode"),
        "{output}"
    );
    assert!(!output.contains("/allow <n>"), "{output}");
    assert!(output.contains("Prefix: /mo"), "{output}");
    assert!(output.contains("Unknown slash command: /modd"), "{output}");
    assert!(
        output.contains("Use /help to see available commands."),
        "{output}"
    );
    assert!(
        output.contains("cosh-osc$ echo after-slash-hint"),
        "{output}"
    );
    assert!(output.contains("after-slash-hint"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash command"), "{output}");
    assert!(!output.contains("bash: /:"), "{output}");
    assert!(!output.contains("bash: /mo"), "{output}");
    assert!(!output.contains("bash: /modd"), "{output}");
}

#[test]
fn raw_cli_slash_cards_wrap_long_text_and_restore_prompt() {
    let output = run_raw_cli_with_env(
        "fake",
        "/asdfasdvavevawwaiodswnvaiosnvaosdnvoasdvpwd\n\
         /help\n\
         echo after-long-slash\n\
         exit\n",
        &[("TERM", "xterm-256color"), ("COSH_SHELL_WIDTH", "72")],
    );

    assert!(output.contains("Unknown slash command"), "{output}");
    assert!(output.contains("Slash commands"), "{output}");
    assert!(
        output.contains("/approval-mode [ask|auto] - alias for /mode"),
        "{output}"
    );
    assert!(
        output.contains("cosh-osc$ echo after-long-slash"),
        "{output}"
    );
    assert!(output.contains("after-long-slash"), "{output}");
    assert_agent_block_width(&output, 72);
    assert!(!output.contains("[ask|auto]alias"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash"), "{output}");
    assert!(!output.contains("bash: /asdf"), "{output}");
}

#[test]
fn raw_cli_mode_slash_updates_approval_mode_with_feedback() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mode auto\n\
         /help\n\
         /approval-mode ask\n\
         /mode invalid\n\
         echo after-mode\n\
         exit\n",
    );

    assert!(output.contains("Approval mode"), "{output}");
    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Mode: auto."), "{output}");
    assert!(output.contains("Mode set to ask."), "{output}");
    assert!(output.contains("Unknown mode: invalid"), "{output}");
    assert!(output.contains("Use /mode ask or /mode auto."), "{output}");
    assert!(output.contains("after-mode"), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
    assert!(!output.contains("bash: /approval-mode"), "{output}");
}

#[test]
fn raw_cli_mode_slash_panel_selects_auto_with_card_input() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"/mode\n".to_vec(), Duration::ZERO),
            (b"\x1b[C\n".to_vec(), Duration::from_millis(300)),
            (b"/help\n".to_vec(), Duration::from_millis(300)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Approval mode"), "{output}");
    assert!(output.contains("Current: ask"), "{output}");
    assert!(output.contains("> [ auto ] Auto-allow"), "{output}");
    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Mode: auto."), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
    assert!(!output.contains("bash: \u{1b}"), "{output}");
}

#[test]
fn raw_cli_auto_mode_runs_safe_bash_tool_without_approval_panel() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mode auto\n\
         ?? request tool approval\n\
         exit\n",
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ git status"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("[ Allow once ]"), "{output}");
    assert!(
        !output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
}

#[test]
fn raw_cli_auto_mode_skips_readonly_builtin_tool_approval_panel() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mode auto\n\
         ?? request readonly builtin tool\n\
         exit\n",
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Auto-approved req-2"), "{output}");
    assert!(output.contains(r#"{"file_path":"Cargo.toml"}"#), "{output}");
    assert!(
        output.contains(r#"{"pattern":"cosh","path":"crates/cosh-shell"}"#),
        "{output}"
    );
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("[ Allow once ]"), "{output}");
    assert!(!output.contains("$ {\"file_path\""), "{output}");
}

#[test]
#[ignore] // timing sensitive
    fn raw_cli_auto_mode_still_asks_for_unsafe_bash_tool() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"/mode auto\n".to_vec(), Duration::ZERO),
            (
                b"?? request unsafe tool approval\n".to_vec(),
                Duration::from_millis(150),
            ),
            (b"\x1b".to_vec(), Duration::from_millis(300)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Approval req-"), "{output}");
    assert!(
        output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
    assert!(output.contains("Cancelled"), "{output}");
    assert!(output.contains("No command ran."), "{output}");
    assert!(!output.contains("Auto-approved"), "{output}");
}

#[test]
fn raw_cli_cancel_is_intercepted_and_keeps_shell_usable() {
    let output = run_raw_cli_with_input("fake", "/cancel\necho after-cancel\nexit\n");

    assert!(output.contains("Agent cancelled"));
    assert!(output.contains("no active Agent run is currently waiting for cancellation"));
    assert!(output.contains("Shell remains active."));
    assert!(output.contains("after-cancel"));
    assert!(!output.contains("bash: /cancel"));
}

#[test]
fn raw_cli_cancel_stops_active_agent_run_and_keeps_shell_usable() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? hold test slow agent\n".to_vec(), Duration::ZERO),
            (b"/cancel\n".to_vec(), Duration::from_millis(500)),
            (
                b"echo after-active-cancel\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert!(output.contains("Agent cancellation requested"));
    assert!(output.contains("Stopping active Agent run"));
    assert!(output.contains("Agent cancelled"));
    assert!(output.contains("Reason: user requested cancellation"));
    assert!(output.contains("after-active-cancel"));
    assert!(!output.contains("bash: /cancel"));
    assert!(!output.contains("Slow fake response for"));
}

#[test]
fn raw_cli_ctrl_c_stops_active_agent_run_and_keeps_shell_usable() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? very slow agent\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(500)),
            (
                b"echo after-agent-ctrl-c\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert!(output.contains("Agent cancellation requested"));
    assert!(output.contains("Stopping active Agent run"));
    assert!(output.contains("Agent cancelled"));
    assert!(output.contains("Reason: user requested cancellation"));
    assert!(output.contains("after-agent-ctrl-c"));
    assert!(!output.contains("Slow fake response for"));
}

#[test]
fn raw_cli_slash_confirmation_is_consumed_once() {
    let output = run_raw_cli_with_input(
        "fake",
        "/explain last error\n\
         ls /path/that/does/not/exist\n\
         ls /another/path/that/does/not/exist\n\
         exit\n",
    );

    assert_eq!(count_occurrences(&output, "The command ls "), 2);
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(output.contains("The command ls /another/path/that/does/not/exist failed"));
}

#[test]
fn raw_cli_clear_cancels_pending_failed_command_analysis() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n/clear\n/explain last error\necho after-clear\nexit\n",
    );

    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed"));
    assert!(output.contains("after-clear"));
}

#[test]
fn raw_cli_shell_cancels_pending_failed_command_analysis() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n/shell\n/explain last error\necho after-shell\nexit\n",
    );

    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed"));
    assert!(output.contains("after-shell"));
}

#[test]
fn raw_cli_natural_language_invokes_claude_adapter_without_failed_command() {
    let input = "\u{4f60}\u{597d}\nexit\n";
    let output = run_raw_cli_with_input("claude", input);

    assert!(output.contains("\u{4f60}\u{597d}"));
    assert!(output.contains("Thinking..."));
    assert!(output.contains("Claude Code adapter prepared a safe recommend-only invocation"));
    assert!(output.contains("claude --print"));
    assert!(!output.contains("command exited with code"));
    assert_inline_before_followup(&output, "Thinking...", "Claude Code adapter");
}

#[test]
fn raw_cli_natural_language_keeps_later_failed_command_auto_analysis() {
    let input = "\u{4f60}\u{597d}\nls /path/that/does/not/exist\nexit\n";
    let output = run_raw_cli_with_input("fake", input);

    assert!(output.contains("Thinking..."));
    assert!(output.contains("Received shell prompt request"));
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed"));
}

#[test]
#[ignore] // card wrap breaks substring
    fn raw_cli_natural_language_includes_recent_shell_context() {
    let output = run_raw_cli_with_input(
        "fake",
        "echo shell-context-ok\n\
         please show context\n\
         exit\n",
    );

    assert!(output.contains("shell-context-ok"), "{output}");
    assert!(
        output.contains("Recent context visible to Agent"),
        "{output}"
    );
    let no_wrap: String = output.replace('│', "");
    assert!(no_wrap.contains("command=echo shell-context-ok"), "{output}");
    assert!(output.contains("ref="), "{output}");
    assert!(!output.contains("command=exit"), "{output}");
}

#[test]
fn raw_cli_natural_language_includes_command_hook_hints() {
    let output = run_raw_cli_with_input(
        "fake",
        "false\n\
         please show context\n\
         exit\n",
    );

    assert!(
        output.contains("Recent context visible to Agent"),
        "{output}"
    );
    assert!(output.contains("Command hook"), "{output}");
    assert!(output.contains("Command result finding"), "{output}");
    assert!(output.contains("false exited with code 1"), "{output}");
    assert!(output.contains("command=false"), "{output}");
    assert!(output.contains("Hook hints"), "{output}");
    assert!(output.contains("command failed; exit=1"), "{output}");
    assert!(
        output.contains("Inspect output_ref before suggesting fixes"),
        "{output}"
    );
    assert!(
        !output.contains("No command ran; Agent actions still require governance."),
        "{output}"
    );
}

#[test]
fn raw_cli_natural_language_after_failure_keeps_failed_command_analysis() {
    let input = "ls /path/that/does/not/exist\n\u{4f60}\u{597d}\nexit\n";
    let output = run_raw_cli_with_input("fake", input);

    assert!(output.contains("Thinking..."));
    assert!(output.contains("Received shell prompt request: \u{4f60}\u{597d}"));
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed"));
}

#[test]
fn raw_cli_failed_command_guidance_appears_before_next_prompt() {
    let output = run_raw_cli_with_input("fake", "ls ccc\nexit\n");

    assert!(output.contains("ls: ccc: No such file or directory"));
    assert!(output.contains("The command ls ccc failed with exit code 1."));
    assert_inline_before_followup(
        &output,
        "The command ls ccc failed with exit code 1.",
        "cosh-osc$ exit",
    );
    assert!(!output.contains("Command failed"));
    assert!(!output.contains("Agent not called"));
    assert!(!output.contains("suggestion: show a short explanation"));
    assert!(!output.contains("`exit` exited with code"));
    assert!(!output.contains("The command exit failed"));
}

#[test]
fn raw_cli_failed_command_invokes_claude_adapter() {
    let output = run_raw_cli_with_input("claude", "ls ccc\nexit\n");

    assert!(output.contains("Claude Code adapter prepared a safe recommend-only invocation"));
    assert!(output.contains("claude --print"));
    assert!(!output.contains("Agent not called"));
}

#[test]
#[ignore] // timing sensitive
    fn raw_cli_failed_command_waits_for_active_agent_then_analyzes() {
    let output = run_raw_cli_with_input(
        "fake",
        "?? slow agent\nls ccc\necho after-queued\nexit\n",
    );

    assert!(output.contains("Agent queued"));
    assert!(output.contains("Captured failed command: ls ccc"));
    assert!(output.contains("Slow fake response for: ?? slow agent"));
    assert!(output.contains("The command ls ccc failed with exit code 1."));
    assert!(output.contains("after-queued"));
}

#[test]
#[ignore] // needs PTY signal fix
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
fn raw_cli_delays_agent_output_while_foreground_command_is_active() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? hold test slow agent\n".to_vec(), Duration::ZERO),
            (
                b"sleep 0.3; echo after-foreground\nexit\n".to_vec(),
                Duration::from_millis(200),
            ),
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
fn raw_cli_slow_agent_shows_elapsed_heartbeat() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? very slow agent\n".to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(2200)),
        ],
    );

    assert!(!output.contains("Still working..."));
    assert!(!output.contains("Phase: thinking"));
    assert!(!output.contains("simulating a slow fake Agent run"));
    assert!(output.contains("Slow fake response for: ?? very slow agent"));
}

#[test]
fn raw_cli_agent_marker_invokes_adapter_without_failed_command() {
    let output = run_raw_cli_with_input("fake", "?? check current directory\nexit\n");

    assert!(output.contains("Thinking..."));
    assert!(output.contains("Received shell prompt request: ?? check current directory"));
    assert!(!output.contains("command exited with code"));
    assert_no_prompt_between(&output, "Thinking...", "Received shell prompt request");
}

#[test]
fn raw_cli_empty_enter_and_ctrl_c_do_not_start_agent() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(50)),
            (b"\nexit\n".to_vec(), Duration::from_millis(50)),
        ],
    );

    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("Command failed:"), "{output}");
    assert!(!output.contains("Agent status"), "{output}");
    assert!(output.contains("cosh-osc$"));
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

    assert_eq!(count_occurrences(&output, "Thinking..."), 1, "{output}");
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

#[test]
fn raw_cli_no_color_keeps_box_layout_when_terminal_supports_it() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\nexit\n",
        &[("NO_COLOR", "1"), ("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"));
    assert!(output.contains("╭─ Recommendations"));
    assert!(!output.contains("╭─ Agent status"));
}

#[test]
fn raw_cli_animation_mode_uses_transient_agent_status() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? slow agent\nexit\n",
        &[
            ("COSH_SHELL_ANIMATION", "always"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(output.contains("⠋ Thinking..."), "{output}");
    assert!(output.contains("\x1b[2K"), "{output}");
    assert!(
        output.contains("Slow fake response for: ?? slow agent"),
        "{output}"
    );
}

#[test]
fn raw_cli_agent_response_renders_markdown_inside_card() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ Project check"), "{output}");
    assert!(output.contains("│ ─────────────────"), "{output}");
    assert!(!output.contains("│ ============="), "{output}");
    assert!(output.contains("│ • Run git status"), "{output}");
    assert!(output.contains("│ • Build workspace"), "{output}");
    assert!(
        output.contains("│   ◦ Use package scoped tests"),
        "{output}"
    );
    assert!(
        output.contains("│   1. Keep shell-first validation repeatable"),
        "{output}"
    );
    assert!(
        output.contains("│ 1. Review rendered transcript"),
        "{output}"
    );
    assert!(output.contains("│ ┌ code: bash"), "{output}");
    assert!(output.contains("│ │ cargo build --workspace"), "{output}");
    assert!(output.contains("│ │ if test -d crates; then"), "{output}");
    assert!(
        output.contains("│ │   cargo test --package cosh-shell"),
        "{output}"
    );
    assert!(output.contains("│ │ fi"), "{output}");
    assert!(
        output.contains("│ │ Commands are suggestions only."),
        "{output}"
    );
    assert!(
        !output.contains("│ > Commands are suggestions only."),
        "{output}"
    );
    assert!(!output.contains("```bash"), "{output}");
    assert!(!output.contains("```"), "{output}");
}

#[test]
fn raw_cli_agent_response_streams_markdown_fragments_inside_card() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? stream markdown\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ Streaming check"), "{output}");
    assert!(output.contains("│ ─────────────────"), "{output}");
    assert!(!output.contains("│ ==============="), "{output}");
    assert!(output.contains("│ • First item"), "{output}");
    assert!(output.contains("│ • Second item"), "{output}");
    assert!(
        output.contains("│ │ cargo test --package cosh-shell"),
        "{output}"
    );
    assert!(output.contains("│ Done."), "{output}");
    assert!(!output.contains("# Streaming check"), "{output}");
    assert!(!output.contains("```bash"), "{output}");
}

#[test]
fn raw_cli_agent_response_streams_markdown_table_as_stable_block() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? stream markdown table\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ Streaming table"), "{output}");
    assert!(output.contains("│ ─────────────────"), "{output}");
    assert!(!output.contains("│ ==============="), "{output}");
    assert!(output.contains("│ ┌ table"), "{output}");
    assert!(output.contains("│ │排名"), "{output}");
    assert!(output.contains("ps aux | grep cosh"), "{output}");
    assert!(output.contains("│ Done."), "{output}");
    assert!(!output.contains("# Streaming table"), "{output}");
    assert!(!output.contains("| --- | --- | --- |"), "{output}");
}

#[test]
fn raw_cli_agent_response_streams_soft_wrapped_markdown_paragraph() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? stream markdown paragraph\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ Streaming paragraph"), "{output}");
    assert!(
        output.contains("This Agent answer starts and continues"),
        "{output}"
    );
    assert!(output.contains("source line with 中文内容."), "{output}");
    assert!(
        !output.contains("starts\n│ and continues on another source line"),
        "{output}"
    );
    assert!(output.contains("│ Done."), "{output}");
}

#[test]
fn raw_cli_agent_response_renders_markdown_table_inside_card() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown table\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ 内存占用 Top 10 分析:"), "{output}");
    assert!(output.contains("│ ┌ table"), "{output}");
    assert!(output.contains("│ │排名"), "{output}");
    assert!(output.contains("│ │1"), "{output}");
    assert!(output.contains("Virtualizatio"), "{output}");
    assert!(
        output.contains("n.VirtualMach") || output.contains("n.VirtualMachine"),
        "{output}"
    );
    assert!(output.contains("ps aux | grep cosh"), "{output}");
    assert!(output.contains("│ 关键发现：Qoder 占用最多。"), "{output}");
    assert!(!output.contains("| --- | --- | --- | --- |"), "{output}");
}

#[test]
fn raw_cli_agent_response_renders_markdown_table_at_configured_narrow_width() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown table\nexit\n",
        &[("TERM", "xterm-256color"), ("COSH_SHELL_WIDTH", "54")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ 内存占用 Top 10 分析:"), "{output}");
    assert!(output.contains("│ ┌ table"), "{output}");
    assert!(output.contains("Virtualizatio"), "{output}");
    assert!(output.contains("VirtualMachine"), "{output}");
    assert!(output.contains("ps aux | grep c"), "{output}");
    assert!(output.contains("osh"), "{output}");
    assert!(output.contains("│ 关键发现：Qoder 占用最多。"), "{output}");
    assert!(!output.contains("| --- | --- | --- | --- |"), "{output}");
    assert_agent_block_width(&output, 54);
}

#[test]
fn raw_cli_agent_response_keeps_markdown_pipe_output_without_separator_as_text() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown pipe output\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ Shell output:"), "{output}");
    assert!(
        output.contains("│ | 1 | Virtualization.VirtualMachine | ~1470 MB |"),
        "{output}"
    );
    assert!(output.contains("│ | 2 | Node | ~572 MB |"), "{output}");
    assert!(output.contains("│ Done."), "{output}");
    assert!(!output.contains("│ ┌ table"), "{output}");
}

#[test]
fn raw_cli_agent_response_renders_indented_markdown_code_inside_card() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown indented code\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ Indented code check"), "{output}");
    assert!(output.contains("│ ┌ code "), "{output}");
    assert!(
        output.contains("│ │ cargo test --package cosh-shell"),
        "{output}"
    );
    assert!(output.contains("│ │ git status --short"), "{output}");
    assert!(output.contains("│ Done."), "{output}");
    assert!(!output.contains("│     cargo test"), "{output}");
    assert!(!output.contains("```"), "{output}");
}

#[test]
fn raw_cli_agent_response_joins_soft_wrapped_markdown_paragraph() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown paragraph\nexit\n",
        &[("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent"), "{output}");
    assert!(output.contains("│ Paragraph rendering"), "{output}");
    assert!(
        output.contains("This Agent answer is split across"),
        "{output}"
    );
    assert!(output.contains("source lines with 中文内容"), "{output}");
    assert!(output.contains("as one"), "{output}");
    assert!(output.contains("Markdown paragraph."), "{output}");
    assert!(
        !output.contains("split\n│ across multiple source lines"),
        "{output}"
    );
}

#[test]
fn raw_cli_agent_response_renders_markdown_in_plain_mode() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown\nexit\n",
        &[("COSH_SHELL_RENDER", "plain"), ("TERM", "xterm-256color")],
    );

    assert!(output.contains("Agent:"), "{output}");
    assert!(output.contains("  Project check"), "{output}");
    assert!(output.contains("  ============="), "{output}");
    assert!(output.contains("  - Run git status"), "{output}");
    assert!(
        output.contains("    1. Keep shell-first validation repeatable"),
        "{output}"
    );
    assert!(
        output.contains("  1. Review rendered transcript"),
        "{output}"
    );
    assert!(output.contains("  +-- code: bash"), "{output}");
    assert!(output.contains("  | cargo build --workspace"), "{output}");
    assert!(output.contains("  | if test -d crates; then"), "{output}");
    assert!(
        output.contains("  |   cargo test --package cosh-shell"),
        "{output}"
    );
    assert!(output.contains("  | fi"), "{output}");
    assert!(!output.contains("# Project check"), "{output}");
    assert!(!output.contains("```bash"), "{output}");
}

#[test]
fn raw_cli_dumb_terminal_uses_plain_blocks() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\nexit\n",
        &[("NO_COLOR", "1"), ("TERM", "dumb")],
    );

    assert_plain_blocks(&output);
}

#[test]
fn raw_cli_explicit_plain_render_mode_uses_plain_blocks() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\nexit\n",
        &[("COSH_SHELL_RENDER", "plain"), ("TERM", "xterm-256color")],
    );

    assert_plain_blocks(&output);
}

fn assert_plain_blocks(output: &str) {
    assert!(output.contains("Agent:"));
    assert!(output.contains("Recommendations:"));
    assert!(!output.contains("Agent status:"));
    assert!(!output.contains('╭'));
    assert!(!output.contains('│'));
    assert!(!output.contains('╰'));
}

fn run_raw_cli(adapter: &str) -> String {
    run_raw_cli_with_input(
        adapter,
        "/explain last error\nls /path/that/does/not/exist\necho after-inline\nexit\n",
    )
}

fn run_raw_cli_with_input(adapter: &str, input: &str) -> String {
    run_raw_cli_with_env(adapter, input, &[])
}

fn run_raw_cli_with_env(adapter: &str, input: &str, envs: &[(&str, &str)]) -> String {
    run_raw_cli_with_args_and_env(adapter, &[], input, envs)
}

fn run_raw_cli_with_args_and_env(
    adapter: &str,
    extra_args: &[&str],
    input: &str,
    envs: &[(&str, &str)],
) -> String {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let mut command = Command::new(binary);
    command
        .args(["raw", adapter])
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn cosh-shell raw");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        stdin
            .write_all(input.as_bytes())
            .expect("write scripted shell input");
    }

    let output = child.wait_with_output().expect("wait raw cli");
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn run_raw_cli_with_delayed_input(adapter: &str, chunks: Vec<(Vec<u8>, Duration)>) -> String {
    run_raw_cli_with_args_and_delayed_input(adapter, &[], chunks)
}

fn run_raw_cli_with_args_and_delayed_input(
    adapter: &str,
    extra_args: &[&str],
    chunks: Vec<(Vec<u8>, Duration)>,
) -> String {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let mut child = Command::new(binary)
        .args(["raw", adapter])
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cosh-shell raw");

    {
        let mut stdin = child.stdin.take().expect("child stdin");
        for (bytes, delay) in chunks {
            thread::sleep(delay);
            stdin.write_all(&bytes).expect("write delayed input");
            stdin.flush().expect("flush delayed input");
        }
    }

    let output = child.wait_with_output().expect("wait raw cli");
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn assert_inline_before_followup(output: &str, inline_marker: &str, followup_output: &str) {
    let inline_pos = output.find(inline_marker).expect("inline guidance marker");
    let followup_pos = output
        .rfind(followup_output)
        .expect("followup shell output");
    assert!(inline_pos < followup_pos, "{output}");
}

fn assert_no_prompt_between(output: &str, start_marker: &str, end_marker: &str) {
    let start = output.find(start_marker).expect("start marker");
    let end = output[start..]
        .find(end_marker)
        .map(|idx| start + idx)
        .expect("end marker");
    assert!(!output[start..end].contains("cosh-osc$"), "{output}");
}

fn count_occurrences(output: &str, needle: &str) -> usize {
    output.match_indices(needle).count()
}

fn assert_agent_block_width(output: &str, max_width: usize) {
    let clean = strip_ansi_escape(output);
    for line in clean
        .lines()
        .flat_map(|line| line.split('\r'))
        .filter(|line| {
            line.contains('╭')
                || line.contains('╰')
                || line.contains('│')
                || line.contains('┌')
                || line.contains('└')
        })
    {
        assert!(
            display_width(line) <= max_width,
            "line width {} exceeds {max_width}: {line:?}\n{output}",
            display_width(line)
        );
    }
}

fn strip_ansi_escape(text: &str) -> String {
    let mut stripped = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            stripped.push(ch);
            continue;
        }

        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    stripped
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| match ch {
            '\t' => 4,
            '─' | '│' | '┌' | '┐' | '└' | '┘' | '╭' | '╮' | '╰' | '╯' | '├' | '┤' | '┬' | '┴'
            | '┼' | '•' | '◦' => 1,
            ch if ch.is_control() => 0,
            ch if ch.is_ascii() => 1,
            _ => 2,
        })
        .sum()
}
