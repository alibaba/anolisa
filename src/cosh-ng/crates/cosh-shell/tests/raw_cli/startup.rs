use super::*;

#[test]
fn raw_cli_startup_banner_renders_when_enabled() {
    let output = run_raw_cli_with_env(
        "fake",
        "exit\n",
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("COSH_SHELL_LANG", "en-US"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(output.contains("cosh-shell"), "{output}");
    assert!(output.contains("Adapter: fake"), "{output}");
    assert!(output.contains("Shell: bash"), "{output}");
    assert!(output.contains("Mode: auto"), "{output}");
    assert!(output.contains("/help"), "{output}");
    assert!(output.contains("/hooks"), "{output}");
    assert!(!output.contains("/explain"), "{output}");
    assert!(
        !output.contains("┌─┐┌─┐┌─┐┬ ┬"),
        "logo should be removed: {output}"
    );
    assert!(
        !output.contains("Agent actions still require approval"),
        "footer should be removed: {output}"
    );
    assert!(
        !output.contains("Startup hooks: none configured"),
        "no hooks line when hooks are disabled: {output}"
    );
    assert!(!output.contains("no command ran"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ cosh-shell"), "{output}");
    assert_inline_before_followup(&output, "╭ cosh-shell", "exit");
    assert!(!output.contains("Thinking..."), "{output}");
}

#[test]
fn raw_cli_startup_banner_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "exit\n",
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("COSH_SHELL_LANG", "zh-CN"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(output.contains("cosh-shell"), "{output}");
    assert!(output.contains("后端: fake"), "{output}");
    assert!(output.contains("Shell: bash"), "{output}");
    assert!(output.contains("模式: auto"), "{output}");
    assert!(output.contains("/help"), "{output}");
}

#[test]
fn raw_cli_startup_hooks_use_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "exit\n",
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("COSH_SHELL_STARTUP_HOOKS", "1"),
            ("COSH_SHELL_LANG", "zh-CN"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(
        output.contains("启动 hooks: 内置只读检查已完成"),
        "{output}"
    );
    assert!(output.contains("启动检查结果"), "{output}");
    assert!(
        output.contains("检测到 Cargo.toml Rust 项目")
            || output.contains("内置只读检查未发现启动项"),
        "{output}"
    );
    assert!(
        output.contains("cosh-shell 只检查了轻量启动上下文"),
        "{output}"
    );
    for label in [
        "Startup hooks:",
        "Startup findings",
        "Rust project detected",
        "No startup findings from built-in read-only checks",
        "only inspected lightweight startup context",
    ] {
        assert!(
            !output.contains(label),
            "startup English UI label leaked: {label}\n{output}"
        );
    }
}

#[test]
fn raw_cli_startup_hooks_no_findings_use_zh_language_env() {
    let cwd = temp_shell_home("startup-hooks-no-findings");
    let output = run_raw_cli_with_args_env_current_dir_and_delayed_input(
        "fake",
        &[],
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("COSH_SHELL_STARTUP_HOOKS", "1"),
            ("COSH_SHELL_LANG", "zh-CN"),
            ("TERM", "xterm-256color"),
        ],
        &cwd,
        vec![(b"exit\n".to_vec(), Duration::ZERO)],
    );

    assert!(
        output.contains("启动 hooks: 内置只读检查已完成"),
        "{output}"
    );
    assert!(output.contains("启动检查结果"), "{output}");
    assert!(output.contains("内置只读检查未发现启动项"), "{output}");
    assert!(
        !output.contains("No startup findings from built-in read-only checks"),
        "{output}"
    );
}

#[test]
fn raw_cli_default_agent_mode_defers_safe_fallback_tool() {
    let home = temp_shell_home("default-agent-auto");
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("COSH_SHELL_LANG", "en-US"),
        ],
        vec![
            (
                b"?? request tool approval\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("Mode: auto"), "{output}");
    assert!(output.contains("Deferred req-1"), "{output}");
    assert!(output.contains("$ git status"), "{output}");
    assert!(!output.contains("Approval req-"), "{output}");
    assert!(!output.contains("[ Allow once ]"), "{output}");
    assert!(!output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        !output.contains("evidence: ShellCommandCompleted"),
        "{output}"
    );
}

#[test]
fn raw_cli_raw_run_without_adapter_uses_cosh_tui_default_adapter() {
    let home = temp_shell_home("cosh-tui-default-adapter");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
case "$init" in
  *'"subtype":"initialize"'*) ;;
  *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-default","is_error":true,"result":"missing initialize"}'; exit 1 ;;
esac
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-default","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-default-adapter-smoke*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-default","message":{"content":[{"type":"text","text":"Cosh-tui default adapter reached via implicit raw."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-default","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-default","is_error":true,"result":"unexpected prompt"}'
"#,
    );

    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_default_with_args_env_and_delayed_input(
        &["--run"],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_STARTUP_BANNER", "1"),
        ],
        vec![
            (
                b"?? cosh-tui-default-adapter-smoke\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Adapter: cosh-tui"), "{output}");
    assert!(
        output.contains("Cosh-tui default adapter reached via implicit raw."),
        "{output}"
    );
    assert!(output.contains("provider invocation:"), "{output}");
    assert!(
        output.contains("cosh-raw-cli-cosh-tui-default-adapter"),
        "{output}"
    );
    assert!(output.contains("/bin/cosh-tui"), "{output}");
    assert!(!output.contains("Adapter: fake"), "{output}");
    assert!(!output.contains("unexpected prompt"), "{output}");
    assert!(!output.contains("failed to run cosh-tui"), "{output}");
}

#[test]
fn raw_cli_startup_hooks_render_markdown_findings_without_running_commands() {
    let output = run_raw_cli_with_env(
        "fake",
        "exit\n",
        &[
            ("COSH_SHELL_STARTUP_BANNER", "1"),
            ("COSH_SHELL_STARTUP_HOOKS", "1"),
            ("COSH_SHELL_LANG", "en-US"),
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
    assert!(
        !output.contains("Read-only startup checks."),
        "hook findings should be inline, not a separate panel: {output}"
    );
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
            ("COSH_SHELL_LANG", "en-US"),
            ("TERM", "xterm-256color"),
        ],
    );

    assert!(output.contains("cosh-shell"), "{output}");
    assert!(output.contains("Shell: zsh"), "{output}");
    assert!(!output.contains("Shell: bash"), "{output}");
    assert!(!output.contains("zsh: command not found"), "{output}");
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
fn raw_cli_adapter_failure_keeps_shell_usable() {
    let output = run_raw_cli_with_input(
        "fake",
        "?? backend unavailable\n\
         echo after-backend-unavailable\n\
         exit 0\n",
    );

    assert!(output.contains("fake backend unavailable"), "{output}");
    assert!(output.contains("after-backend-unavailable"), "{output}");
    assert!(!output.contains("bash: ??"), "{output}");
    assert!(
        !output.contains("The command ?? backend unavailable failed"),
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
}

#[test]
fn raw_cli_backend_unavailable_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? backend unavailable\n\
         echo after-backend-unavailable\n\
         exit 0\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("正在思考..."), "{output}");
    assert!(output.contains("Agent 回复:"), "{output}");
    assert!(output.contains("治理:"), "{output}");
    assert!(output.contains("fake backend unavailable"), "{output}");
    assert!(output.contains("after-backend-unavailable"), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("Agent response:"), "{output}");
    assert!(!output.contains("bash: ??"), "{output}");
    assert!(
        !output.contains("The command ?? backend unavailable failed"),
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
}

#[test]
fn raw_cli_adapter_error_keeps_shell_usable() {
    let output = run_raw_cli_with_input(
        "fake",
        "?? adapter crash\n\
         echo after-adapter-crash\n\
         exit 0\n",
    );

    assert!(output.contains("fake adapter crashed"), "{output}");
    assert!(output.contains("after-adapter-crash"), "{output}");
    assert!(!output.contains("bash: ??"), "{output}");
    assert!(
        !output.contains("The command ?? adapter crash failed"),
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
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
