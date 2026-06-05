use std::fs;
use std::io::Write;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use wait_timeout::ChildExt;

#[path = "raw_cli/approval.rs"]
mod approval;
#[path = "raw_cli/question.rs"]
mod question;
#[path = "support/mod.rs"]
mod support;

use support::raw_cli::*;

const APPROVAL_ZH_FORBIDDEN_UI: &[&str] = &[
    "Approval required",
    "Subject: Bash",
    "Tool input:",
    "Allow once",
    "Always trust",
    "Approved req-1",
    "Bash tool sent to shell",
    "Approval details",
    "Approval journal",
    "Policy: user approval is required",
    "Keys:",
    "Command block:",
    "Redaction: ref_only",
];

const DETAILS_ZH_FORBIDDEN_UI: &[&str] = &[
    "Activity details",
    "Details unavailable",
    " is not available; use a Details action",
    "Run:",
    "Detail:",
    "Tool output - stdout captured; [Details]",
];

const PROVIDER_NATIVE_ZH_FORBIDDEN_UI: &[&str] = &[
    "Provider-native shell tool allowed",
    "Read-only tools auto-approved; risky requests need confirmation.",
    "run_shell_command requested; [Details]",
    "Tool output - stdout captured; [Details]",
];

const RENDERER_ZH_FORBIDDEN_UI: &[&str] = &[
    "╭ Agent ─",
    "│ ┌ code:",
    "│ ┌ table",
    "No selectable recommendation",
    "No selectable recommendation is available yet",
];

const QUESTION_ZH_FORBIDDEN_UI: &[&str] = &[
    "Agent question",
    "Select one:",
    "Left/Right move",
    "Answer:",
    "Answer sent",
    "Sent to Agent",
];

const SLASH_CONFIG_ZH_FORBIDDEN_UI: &[&str] = &[
    "Slash commands",
    "Slash command hint",
    "Unknown slash command",
    "Did you mean /help?",
    "Use /help to see available commands.",
    "User mode",
    "Invalid language",
    "Unknown config key",
    "Config saved",
    "language is a persistent config",
    "Use /config language [auto|en-US|zh-CN].",
];

const MODE_ZH_FORBIDDEN_UI: &[&str] = &[
    "Trust confirmation required",
    "Trust mode auto-approves provider tool requests",
    "Run /mode approval trust confirm to enable it explicitly.",
    "Recommend or auto mode remains active until confirmation.",
    "User mode",
    "Current: auto",
    "Explain and suggest only",
    "Read-only auto-approved; risky needs confirmation",
    "All tools auto-approved with audit trail",
    "Keys: Left/Right select",
    "Mode set to trust.",
    "Mode set to recommend.",
    "Hooks evaluate on failure; Agent auto-triggered for failed commands.",
    "Hooks and automatic analysis disabled; use slash commands to trigger.",
];

fn assert_no_migrated_english_ui_labels(output: &str, labels: &[&str]) {
    for label in labels {
        assert!(
            !output.contains(label),
            "migrated English UI label leaked: {label}\n{output}"
        );
    }
}

fn assert_agent_loading_visible(output: &str) {
    assert!(
        output.contains("Thinking...") || output.contains("正在思考..."),
        "{output}"
    );
}

fn agent_loading_count(output: &str) -> usize {
    count_occurrences(output, "Thinking...") + count_occurrences(output, "正在思考...")
}

fn process_is_alive(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn read_pid_file_with_retry(path: &Path) -> u32 {
    let mut last_error = None;
    for _ in 0..40 {
        match fs::read_to_string(path) {
            Ok(pid_text) => return pid_text.trim().parse::<u32>().expect("provider pid"),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    panic!("provider pid file {}: {last_error:?}", path.display());
}

fn signal_process_group(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([format!("-{signal}"), format!("-{pid}")])
        .status();
}

fn signal_pid(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([format!("-{signal}"), pid.to_string()])
        .status();
}

#[test]
fn raw_cli_inline_guidance_works_with_fake_adapter() {
    let output = run_raw_cli_with_envs("fake", &[("COSH_SHELL_LANG", "en-US")]);

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
fn raw_cli_cosh_tui_bash_ordinary_commands_passthrough_without_agent() {
    let home = temp_shell_home("cosh-tui-bash-passthrough");
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_and_env(
        "cosh-tui",
        &["--shell", "bash"],
        "printf 'bash-pwd:%s\\n' \"$PWD\"\necho cosh-pass-bash\nexit\n",
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", "/tmp/cosh-tui-should-not-start"),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("bash-pwd:"), "{output}");
    assert!(output.contains("cosh-pass-bash"), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("failed to run cosh-tui"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_zsh_ordinary_commands_passthrough_without_agent() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_shell_home("cosh-tui-zsh-passthrough");
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_and_env(
        "cosh-tui",
        &["--shell", "zsh"],
        "print -r -- zsh-pwd:$PWD\necho cosh-pass-zsh\nexit\n",
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", "/tmp/cosh-tui-should-not-start"),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("zsh-pwd:"), "{output}");
    assert!(output.contains("cosh-pass-zsh"), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
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
fn raw_cli_zsh_native_loads_existing_user_history() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_zsh_home("native-history");
    let history_file = home.join(".zsh_history");
    fs::write(
        home.join(".zshrc"),
        "HISTSIZE=1000\nSAVEHIST=1000\nsetopt appendhistory\n",
    )
    .unwrap();
    fs::write(&history_file, "echo old-cosh-zsh-history\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let history_str = history_file.to_string_lossy().to_string();

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[
            ("HOME", &home_str),
            ("TERM", "xterm-256color"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (
                b"printf 'histfile:%s\\n' \"$HISTFILE\"\n".to_vec(),
                Duration::ZERO,
            ),
            (b"history\n".to_vec(), Duration::from_millis(150)),
            (
                b"echo new-cosh-zsh-history\n".to_vec(),
                Duration::from_millis(150),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(150)),
        ],
    );

    assert!(
        output.contains(&format!("histfile:{history_str}")),
        "{output}"
    );
    assert!(output.contains("old-cosh-zsh-history"), "{output}");
    assert!(fs::read_to_string(&history_file)
        .unwrap()
        .contains("new-cosh-zsh-history"));
}

#[test]
fn raw_cli_bash_native_loads_existing_user_history() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let home = temp_shell_home("native-bash-history");
    let history_file = home.join(".bash_history");
    fs::write(
        home.join(".bashrc"),
        "export HISTFILE=$HOME/.bash_history\nexport HISTSIZE=1000\nshopt -s histappend\n",
    )
    .unwrap();
    fs::write(&history_file, "echo old-cosh-bash-history\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let history_str = history_file.to_string_lossy().to_string();

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "bash"],
        &[
            ("HOME", &home_str),
            ("TERM", "xterm-256color"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (
                b"printf 'histfile:%s\\n' \"$HISTFILE\"\n".to_vec(),
                Duration::ZERO,
            ),
            (b"history\n".to_vec(), Duration::from_millis(150)),
            (
                b"echo new-cosh-bash-history\n".to_vec(),
                Duration::from_millis(150),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(150)),
        ],
    );

    assert!(
        output.contains(&format!("histfile:{history_str}")),
        "{output}"
    );
    assert!(output.contains("old-cosh-bash-history"), "{output}");
    assert!(fs::read_to_string(&history_file)
        .unwrap()
        .contains("new-cosh-bash-history"));
}

#[test]
fn raw_cli_unsupported_shell_reports_error_without_starting_bash() {
    assert_raw_cli_rejects_shell_args(
        &["raw", "fake", "--shell", "fish"],
        "unsupported raw shell: fish; supported shells: bash, zsh",
    );
}

#[test]
fn raw_cli_double_dash_passthrough_executes_command_directly() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(["--", "echo", "ok"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run double dash passthrough");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(stdout.trim(), "ok", "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.is_empty(), "stdout={stdout}\nstderr={stderr}");
}

#[test]
fn raw_cli_double_dash_passthrough_preserves_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(["--", "sh", "-c", "exit 43"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run direct command with nonzero exit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(43),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Agent:"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_double_dash_passthrough_does_not_capture_child_help_arg() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(["--", "echo", "--help"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run direct command with child help arg");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(stdout.trim(), "--help", "stdout={stdout}\nstderr={stderr}");
    assert!(
        !stderr.contains("Usage: cosh-shell"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_double_dash_passthrough_requires_command() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .arg("--")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run missing direct command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stderr.contains("missing command after --"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_dash_c_passthrough_preserves_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(["-c", "exit 42"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run dash-c passthrough with nonzero exit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(42),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Agent:"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_dash_c_passthrough_filters_wrapper_shell_option() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(["--shell", "bash", "-c", "echo shell-filter-ok"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run dash-c passthrough with shell option");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("shell-filter-ok"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stderr.contains("invalid option"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stderr.contains("--shell"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_stdin_passthrough_preserves_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin passthrough");

    {
        let mut stdin = child.stdin.take().expect("child stdin");
        stdin
            .write_all(b"exit 44\n")
            .expect("write stdin passthrough command");
    }

    let output = child.wait_with_output().expect("wait stdin passthrough");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(44),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Agent:"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_login_dash_c_passthrough_executes_without_agent_ui() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(["--login", "-c", "echo login-c-ok"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run login dash-c passthrough");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("login-c-ok"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("cosh-osc$"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_login_dash_c_passthrough_preserves_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .args(["--login", "-c", "exit 45"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run login dash-c passthrough with nonzero exit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(45),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("cosh-osc$"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_login_argv0_dash_c_passthrough_executes_without_agent_ui() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .arg0("-cosh-shell")
        .args(["-c", "echo argv0-login-c-ok"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run login argv0 dash-c passthrough");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("argv0-login-c-ok"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("cosh-osc$"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_login_argv0_dash_c_passthrough_preserves_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = Command::new(binary)
        .arg0("-cosh-shell")
        .args(["-c", "exit 46"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run login argv0 dash-c passthrough with nonzero exit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(46),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("cosh-osc$"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_login_argv0_stdin_passthrough_preserves_exit_status_without_agent_ui() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let mut child = Command::new(binary)
        .arg0("-cosh-shell")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn login argv0 stdin passthrough");

    {
        let mut stdin = child.stdin.take().expect("child stdin");
        stdin
            .write_all(b"echo argv0-stdin-ok\nexit 47\n")
            .expect("write login argv0 stdin passthrough commands");
    }

    let output = child
        .wait_with_output()
        .expect("wait login argv0 stdin passthrough");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(47),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("argv0-stdin-ok"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("cosh-osc$"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Agent:"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_ai_off_consumes_agent_marker_without_adapter_or_shell_error() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? should not trigger\necho after-ai-off\nexit\n",
        &[("COSH_SHELL_AI", "off"), ("COSH_SHELL_ISOLATED", "1")],
    );

    assert!(output.contains("after-ai-off"), "{output}");
    assert!(!output.contains("Agent:"), "{output}");
    assert!(!output.contains("Thinking..."), "{output}");
    assert!(!output.contains("command not found: ??"), "{output}");
    assert!(!output.contains("bash: ??"), "{output}");
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
fn raw_cli_zsh_shell_arg_intercepts_fragmented_slash() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("COSH_SHELL_LANG", "en-US")],
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
    assert!(
        output.contains("/mode approval [recommend|auto|trust]"),
        "{output}"
    );
    assert!(output.contains("after-zsh-slash"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory: /help"),
        "{output}"
    );
}

#[test]
fn raw_cli_zsh_fragmented_mode_slash_does_not_accumulate_redraws() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        vec![
            (b"/m".to_vec(), Duration::ZERO),
            (b"o".to_vec(), Duration::from_millis(50)),
            (b"d".to_vec(), Duration::from_millis(50)),
            (b"e approval auto\n".to_vec(), Duration::from_millis(50)),
            (b"exit\n".to_vec(), Duration::from_millis(150)),
        ],
    );

    assert!(output.contains("Approval mode"), "{output}");
    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(
        output.contains("Read-only tools auto-approved; risky requests need confirmation."),
        "{output}"
    );
    assert!(!output.contains("/m/mo"), "{output}");
    assert!(!output.contains("/mo/mod"), "{output}");
    assert!(!output.contains("/mod/mode"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory: /mode"),
        "{output}"
    );
}

#[test]
fn raw_cli_zsh_native_known_slash_does_not_reach_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("COSH_SHELL_ISOLATED", "0")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"echo after-native-mode\n".to_vec(),
                Duration::from_millis(150),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("Approval mode"), "{output}");
    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("after-native-mode"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory: /mode"),
        "{output}"
    );
}

#[test]
fn raw_cli_zsh_native_pasted_mode_slash_does_not_reach_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("COSH_SHELL_ISOLATED", "0")],
        vec![
            (
                b"\x1b[200~/mode approval recommend\n\x1b[201~".to_vec(),
                Duration::ZERO,
            ),
            (
                b"echo after-native-pasted-mode\n".to_vec(),
                Duration::from_millis(200),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("Approval mode"), "{output}");
    assert!(output.contains("Mode set to recommend."), "{output}");
    assert!(output.contains("after-native-pasted-mode"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory: /mode"),
        "{output}"
    );
}

#[test]
fn raw_cli_pasted_trust_confirm_sets_trust_mode() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"\x1b[200~/mode approval trust confirm\n\x1b[201~".to_vec(),
                Duration::ZERO,
            ),
            (b"/help\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("Approval mode"), "{output}");
    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Mode: trust. Strategy: smart."), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
}

#[test]
#[ignore = "native zsh completion can invoke user rc and real editor; keep out of default raw_cli"]
fn raw_cli_zsh_native_path_slash_and_tab_stay_in_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("COSH_SHELL_ISOLATED", "0")],
        vec![
            (b"/Users".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(100)),
            (b"vim .".to_vec(), Duration::from_millis(100)),
            (b"/".to_vec(), Duration::from_millis(50)),
            (b"\t".to_vec(), Duration::from_millis(50)),
            (vec![0x03], Duration::from_millis(100)),
            (
                b"echo after-native-tab\n".to_vec(),
                Duration::from_millis(100),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("after-native-tab"), "{output}");
    assert!(!output.contains("Slash command hint"), "{output}");
    assert!(!output.contains("Slash commands"), "{output}");
    assert!(!output.contains("User mode"), "{output}");
    assert!(!output.contains("/mode [recommend|agent]"), "{output}");
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
fn raw_cli_slash_after_failed_command_invokes_adapter() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\n/explain last error\necho after-explain\nexit 0\n",
        &[("COSH_SHELL_LANG", "en-US")],
    );

    assert_agent_loading_visible(&output);
    assert!(output.contains("The command ls /path/that/does/not/exist failed"));
    assert!(!output.contains("Command failed:"), "{output}");
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
         exit 0\n",
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
fn raw_cli_zh_selects_recommendation_without_executing_it() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\n\
         /explain last error\n\
         /select 2\n\
         echo after-select\n\
         exit 0\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("推荐"), "{output}");
    assert!(
        output.contains("[Copy] [Insert] [Details] - 仅展示"),
        "{output}"
    );
    assert!(output.contains("已选择推荐 2"), "{output}");
    assert!(output.contains("echo $PATH"), "{output}");
    assert!(output.contains("仅展示：命令未执行；复制或重新输入后才会运行"));
    assert!(output.contains("after-select"));
    assert!(!output.contains("/.cargo/bin"));
}

#[test]
fn raw_cli_copy_fallback_shows_recommendation_without_executing_it() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"/explain last error\n".to_vec(), Duration::ZERO),
            (
                b"ls /path/that/does/not/exist\n".to_vec(),
                Duration::from_millis(100),
            ),
            (b"/copy 1\n".to_vec(), Duration::from_millis(1_200)),
            (b"echo after-copy\n".to_vec(), Duration::from_millis(200)),
            (b"exit 0\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("Recommendation copy"));
    assert!(output.contains("Copy recommendation 1"));
    assert!(output.contains("pwd"));
    assert!(output.contains("Copy-only: command was shown for copying; it was not executed."));
    assert!(output.contains("after-copy"));
    assert!(!output.contains("bash: /copy"));
}

#[test]
fn raw_cli_details_for_activity_uses_structured_panel() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"/details out-1\n".to_vec(), Duration::from_millis(1_200)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("Activity details out-1"), "{output}");
    assert!(output.contains("Tool output - stdout captured"), "{output}");
    assert!(output.contains("Run: fake-run-input-2"), "{output}");
    assert!(output.contains("Detail:"), "{output}");
    assert!(output.contains("tool: tool-1"), "{output}");
    assert!(output.contains("stream: stdout"), "{output}");
    assert!(output.contains("line 24: fake tool output"), "{output}");
    assert!(!output.contains("Skill loaded: git-project"), "{output}");
    assert!(
        !output.contains("Tool output: stdout captured; [Details] out-1"),
        "{output}"
    );
    assert!(!output.contains("Tool completed"), "{output}");
    assert!(!output.contains("skill-2 skill:"), "{output}");
    assert!(!output.contains("out-1 output:"), "{output}");
    assert!(!output.contains("tool-1 tool:"), "{output}");
    assert!(!output.contains("id: out-1"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_activity_details_uses_zh_language_env() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"?? request tool approval\n".to_vec(), Duration::ZERO),
            (b"/details out-1\n".to_vec(), Duration::from_millis(1_200)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("活动详情 out-1"), "{output}");
    assert!(output.contains("Tool 输出 - stdout 已捕获"), "{output}");
    assert!(output.contains("运行: fake-run-input-2"), "{output}");
    assert!(output.contains("详情:"), "{output}");
    assert!(output.contains("tool: tool-1"), "{output}");
    assert!(output.contains("stream: stdout"), "{output}");
    assert!(output.contains("line 24: fake tool output"), "{output}");
    assert!(!output.contains("Activity details out-1"), "{output}");
    assert!(!output.contains("output - stdout captured"), "{output}");
    assert!(!output.contains("Run: fake-run-input-2"), "{output}");
    assert!(!output.contains("Detail:"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
    assert_no_migrated_english_ui_labels(&output, DETAILS_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_control_shell_permission_uses_foreground_and_suppresses_provider_output() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? provider native tool\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(2_500)),
            (
                b"/details tool-1\n/details out-1\n/details approvals\nexit\n".to_vec(),
                Duration::from_millis(4_500),
            ),
        ],
    );

    assert!(output.contains("Approval required"), "{output}");
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("provider-shell-handoff"), "{output}");
    assert!(output.contains("Activity details tool-1"), "{output}");
    assert!(
        output.contains("Tool - Bash requested: $ printf 'provider-shell-handoff"),
        "{output}"
    );
    assert!(output.contains("Details unavailable:"), "{output}");
    assert!(output.contains("out-1 is not available"), "{output}");
    assert!(
        output.contains("Execution: foreground_shell_pty"),
        "{output}"
    );
    assert!(!output.contains("Activity details out-1"), "{output}");
    assert!(
        !output.contains("PROVIDER NATIVE OUTPUT RENDERED AFTER ALLOW"),
        "{output}"
    );
    assert!(
        !output.contains("Tool output - stdout captured"),
        "{output}"
    );
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_provider_foreground_memory_hook_is_internal() {
    let fixture = temp_shell_home("provider-memory-hook-internal");
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
        &[("PATH", path.as_str())],
        vec![
            (b"?? provider memory hook shell\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(2_500)),
            (b"exit\n".to_vec(), Duration::from_millis(4_500)),
        ],
    );
    let _ = fs::remove_dir_all(&fixture);

    assert!(
        output.contains("Approved req-1") || output.contains("Auto-approved req-1"),
        "{output}"
    );
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("Mem:"), "{output}");
    assert!(!output.contains("Available memory is low"), "{output}");
    assert!(!output.contains("[Analyze] [Ignore]"), "{output}");
    assert!(!output.contains("Hook finding"), "{output}");
    assert!(
        !output.contains("PROVIDER MEMORY NATIVE OUTPUT SHOULD NOT RENDER AFTER ALLOW"),
        "{output}"
    );
}

#[test]
fn raw_cli_agent_fallback_memory_hook_is_internal() {
    let fixture = temp_shell_home("agent-fallback-memory-hook-internal");
    let bin_dir = fixture.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let marker = Path::new("/tmp/cosh-shell-fake-memory-hook-marker");
    let _ = fs::remove_file(marker);
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
        &[("PATH", path.as_str())],
        vec![
            (b"?? agent memory hook fallback\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(2_500)),
            (b"exit\n".to_vec(), Duration::from_millis(4_500)),
        ],
    );
    let _ = fs::remove_dir_all(&fixture);
    let _ = fs::remove_file(marker);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("Mem:"), "{output}");
    assert!(!output.contains("Available memory is low"), "{output}");
    assert!(!output.contains("[Analyze] [Ignore]"), "{output}");
    assert!(!output.contains("Hook finding"), "{output}");
}

#[test]
fn raw_cli_auto_provider_shell_permission_uses_foreground_handoff() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider auto safe shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(
        !output.contains("PROVIDER AUTO NATIVE OUTPUT RENDERED AFTER ALLOW"),
        "{output}"
    );
}

#[test]
fn raw_cli_control_shell_output_uses_foreground_transcript_by_default() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider auto safe shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        !output.contains("PROVIDER AUTO NATIVE OUTPUT RENDERED AFTER ALLOW"),
        "{output}"
    );
    assert!(
        !output.contains("Tool output: stdout captured; [Details] out-1"),
        "{output}"
    );
    assert!(!output.contains("Tool success"), "{output}");
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
}

#[test]
fn raw_cli_zh_control_shell_foreground_localizes_shell_owned_wrapper() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider auto safe shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );

    assert!(output.contains("模式已设置为 auto。"), "{output}");
    assert!(
        output.contains("只读工具会自动批准；高风险请求仍需确认。"),
        "{output}"
    );
    assert!(output.contains("已自动批准 req-1"), "{output}");
    assert!(output.contains("Bash tool 已发送到 shell"), "{output}");
    assert!(
        !output.contains("PROVIDER AUTO NATIVE OUTPUT RENDERED AFTER ALLOW"),
        "{output}"
    );
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(!output.contains("Activity details out-1"), "{output}");
    assert!(
        !output.contains("Read-only tools auto-approved; risky requests need confirmation."),
        "{output}"
    );
    assert!(
        !output.contains("已允许 provider-native shell tool 执行"),
        "{output}"
    );
    assert_no_migrated_english_ui_labels(&output, PROVIDER_NATIVE_ZH_FORBIDDEN_UI);
    assert_no_migrated_english_ui_labels(&output, DETAILS_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_zh_control_shell_details_localizes_shell_owned_chrome() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"?? provider native tool\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(2_500)),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(3_000)),
            (b"/details out-1\n".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert!(output.contains("已批准 req-1"), "{output}");
    assert!(output.contains("Bash tool 已发送到 shell"), "{output}");
    assert!(output.contains("活动详情 tool-1"), "{output}");
    assert!(
        output.contains("run_shell_command 请求审批：$ printf 'provider-shell-handoff"),
        "{output}"
    );
    assert!(output.contains("详情不可用:"), "{output}");
    assert!(output.contains("out-1 不可用"), "{output}");
    assert!(output.contains("evidence: ProviderToolRequest"), "{output}");
    assert!(
        output.contains("execution_path: provider_control_protocol"),
        "{output}"
    );
    assert!(output.contains("request_id: ctrl-1"), "{output}");
    assert!(output.contains("tool_use_id: toolu-1"), "{output}");
    assert!(!output.contains("活动详情 out-1"), "{output}");
    assert!(
        !output.contains("PROVIDER NATIVE OUTPUT RENDERED AFTER ALLOW"),
        "{output}"
    );
    assert!(!output.contains("Tool 输出 - stdout 已捕获"), "{output}");
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(
        !output.contains("已允许 provider-native shell tool 执行"),
        "{output}"
    );
    assert!(!output.contains("Activity details tool-1"), "{output}");
    assert!(
        !output.contains("run_shell_command requested: $ printf 'provider-shell-handoff"),
        "{output}"
    );
    assert!(!output.contains("Activity details out-1"), "{output}");
    assert!(
        !output.contains("Tool output - stdout captured; [Details] out-1"),
        "{output}"
    );
    assert!(!output.contains("bash: /details"), "{output}");
    assert_no_migrated_english_ui_labels(&output, PROVIDER_NATIVE_ZH_FORBIDDEN_UI);
    assert_no_migrated_english_ui_labels(&output, DETAILS_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_claude_without_host_executed_capability_uses_foreground_recovery() {
    let home = temp_shell_home("claude-provider-native-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let claude_path = bin_dir.join("claude");
    write_executable(
        &claude_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-claude-native-fallback","model":"claude-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-claude-native-fallback","message":{"content":[{"type":"text","text":"Claude foreground recovery received shell evidence."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-claude-native-fallback","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-claude-native-fallback","model":"claude-test"}'
read -r user_message
case "$user_message" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-claude-native-fallback","message":{"content":[{"type":"text","text":"Claude foreground recovery received shell evidence."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-claude-native-fallback","is_error":false,"result":"done"}'
    exit 0
    ;;
  *claude-provider-native-fallback*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-claude-shell","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo CLAUDE_NATIVE"},"tool_use_id":"toolu-claude-shell"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ctrl-claude-shell"'*'"behavior":"allow"'*CLAUDE_NATIVE*)
          printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-claude-native-fallback","is_error":true,"result":"unexpected provider-native allow"}'
          exit 1
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-claude-native-fallback","is_error":true,"result":"missing claude allow response"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-claude-native-fallback","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "claude",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"claude-provider-native-fallback\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(3_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("Claude foreground recovery received shell evidence."),
        "{output}"
    );
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(output.contains("cosh-osc$ echo CLAUDE_NATIVE"), "{output}");
    assert!(
        !output.contains("missing claude allow response"),
        "{output}"
    );
}

#[test]
fn raw_cli_host_executed_shell_result_continues_same_provider_turn() {
    let home = temp_shell_home("qwen-host-executed-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let ssh_path = bin_dir.join("ssh");
    write_executable(
        &ssh_path,
        "#!/bin/sh\nprintf '%s\\n' 'OpenSSH_test foreground handoff'\n",
    );
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'df -h'*)
          printf '%s\n' '{"type":"user","session_id":"sess-host-executed","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu-1","is_error":false,"content":"PROVIDER_ECHO_SHOULD_NOT_RENDER_AS_ACTIVITY\n"}]}}'
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed","message":{"content":[{"type":"text","text":"Host-executed shell result received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed","is_error":true,"result":"missing host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed","is_error":false,"result":"ignored"}'
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
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-host-executed-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        !output.contains("Tool output: stdout captured; [Details]"),
        "{output}"
    );
    assert!(
        !output.contains("Tool 输出: stdout 已捕获；[Details]"),
        "{output}"
    );
    assert!(
        !output.contains("PROVIDER_ECHO_SHOULD_NOT_RENDER_AS_ACTIVITY"),
        "{output}"
    );
    assert!(
        output.contains("Host-executed shell result received in same provider turn."),
        "{output}"
    );
    assert!(
        !output.contains("missing host_executed_shell result"),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains(
            "path_selection_reason: provider advertised host-executed shell result support"
        ),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(!output.contains("output_ref:"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_host_executed_streaming_order_renders_shell_before_post_text() {
    let home = temp_shell_home("qwen-host-executed-streaming-order");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-stream-order","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *host-executed-stream-order*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-stream-order","message":{"content":[{"type":"text","text":"HOST EXECUTED PRE TEXT STREAMS"}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'df -h'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-stream-order","message":{"content":[{"type":"text","text":"HOST EXECUTED POST TEXT WAITS"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-stream-order","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-stream-order","is_error":true,"result":"missing host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-stream-order","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"host-executed-stream-order\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(2_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(
        !output.contains("missing host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent 恢复"), "{output}");
    assert_ordered(
        &normalized,
        &[
            "HOST EXECUTED PRE TEXT STREAMS",
            "$ df -h",
            "Filesystem",
            "HOST EXECUTED POST TEXT WAITS",
        ],
    );
}

#[test]
fn raw_cli_cosh_tui_host_executed_shell_result_continues_same_provider_turn() {
    let home = temp_shell_home("cosh-tui-host-executed-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-1","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-1"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'df -h'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed","message":{"content":[{"type":"text","text":"Cosh-tui host-executed shell result received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed","is_error":true,"result":"missing cosh-tui host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-provider-host-executed-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        output.contains("Cosh-tui host-executed shell result received in same provider turn."),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains(
            "path_selection_reason: provider advertised host-executed shell result support"
        ),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(
        output.contains("host-executed shell result: delivered"),
        "{output}"
    );
    assert!(
        output
            .contains("selected shell execution path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains("latest provider request: ctrl-cosh-tui-1"),
        "{output}"
    );
    assert!(
        output.contains("latest tool use id: toolu-cosh-tui-1"),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(
        !output.contains("missing cosh-tui host_executed_shell result"),
        "{output}"
    );
    assert!(
        !output.contains("bash: cosh-tui-provider-host-executed-shell: command not found"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_suppresses_provider_native_echo_after_manual_host_executed_shell() {
    let home = temp_shell_home("cosh-tui-manual-host-executed-echo");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-manual-host-executed-echo","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-manual-host-executed-echo*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-manual-echo","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"sudo -V"},"tool_use_id":"toolu-cosh-tui-manual-echo"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'sudo -V'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-manual-host-executed-echo","message":{"content":[{"type":"text","text":"Manual host-executed result accepted."},{"type":"tool_use","id":"toolu-cosh-tui-manual-echo-provider","name":"shell","input":{"command":"sudo -V"}}]}}'
          printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-manual-host-executed-echo","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu-cosh-tui-manual-echo-provider","is_error":false,"content":"PROVIDER ECHO SHOULD BE SUPPRESSED\n"}]}}'
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-manual-host-executed-echo","message":{"content":[{"type":"text","text":"COSH TUI HOST EXECUTED ECHO FINAL"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":false,"result":"done"}'
          exit 0
          ;;
        *'"behavior":"allow"'*)
          printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":true,"result":"unexpected provider-native allow"}'
          exit 1
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":true,"result":"missing manual host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-manual-host-executed-echo","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (
                b"?? cosh-tui-manual-host-executed-echo\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (b"/debug session\n".to_vec(), Duration::from_millis(6_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-2"), "{output}");
    assert!(!output.contains("auto-approved by provider"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(normalized.contains("\nsudo -V\n"), "{output}");
    assert!(
        output.contains("COSH TUI HOST EXECUTED ECHO FINAL"),
        "{output}"
    );
    assert!(
        !output.contains("PROVIDER ECHO SHOULD BE SUPPRESSED"),
        "{output}"
    );
    assert!(
        !output.contains("unexpected provider-native allow"),
        "{output}"
    );
    assert!(
        !output.contains("missing manual host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

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
fn raw_cli_cosh_tui_auto_safe_shell_auto_approves_host_executed() {
    let home = temp_shell_home("cosh-tui-auto-safe-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-auto-safe","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-auto-safe-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-auto-safe","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-auto-safe"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'df -h'*'Filesystem'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-auto-safe","message":{"content":[{"type":"text","text":"AUTO SAFE HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-auto-safe","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-auto-safe","is_error":true,"result":"missing auto host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-auto-safe","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-auto-safe-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(output.contains("AUTO SAFE HOSTEXEC RECEIVED"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(
        !output.contains("missing auto host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_tui_trust_confirm_shell_auto_approves_host_executed() {
    let home = temp_shell_home("cosh-tui-trust-confirm-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-trust-confirm","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-trust-confirm-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-trust-confirm","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"printf trust-confirm-hostexec"},"tool_use_id":"toolu-trust-confirm"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'printf trust-confirm-hostexec'*'trust-confirm-hostexec'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-trust-confirm","message":{"content":[{"type":"text","text":"TRUST CONFIRM HOSTEXEC RECEIVED"}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-trust-confirm","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-trust-confirm","is_error":true,"result":"missing trust host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-trust-confirm","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (b"/mode approval trust confirm\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-trust-confirm-shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(
        output.contains("$ printf trust-confirm-hostexec"),
        "{output}"
    );
    assert!(output.contains("trust-confirm-hostexec"), "{output}");
    assert!(
        output.contains("TRUST CONFIRM HOSTEXEC RECEIVED"),
        "{output}"
    );
    assert!(!output.contains("Approval required"), "{output}");
    assert!(
        !output.contains("missing trust host_executed_shell result"),
        "{output}"
    );
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
fn raw_cli_cosh_tui_question_card_answer_continues_same_provider_turn() {
    let home = temp_shell_home("cosh-tui-question-answer");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-question","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-question-card*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-question","message":{"content":[{"type":"tool_use","id":"toolu-cosh-tui-ask","name":"ask_user_question","input":{"question":"Choose a color for cosh-tui provider follow-up","options":[{"label":"Green"},{"label":"Blue"}],"allow_free_text":true}}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ask-cosh-tui-1","request":{"subtype":"ask_user","question":"Choose a color for cosh-tui provider follow-up","options":[{"label":"Green"},{"label":"Blue"}],"allow_free_text":true,"multi_select":false}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ask-cosh-tui-1"'*'"answer":"Green"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-question","message":{"content":[{"type":"text","text":"Cosh-tui question answer received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-question","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-question","is_error":true,"result":"missing cosh-tui question answer"}'
    exit 1
    ;;
  *"Answer to pending Agent question"*)
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-question","is_error":true,"result":"question answer restarted provider instead of answering same turn"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-question","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (
                b"?? cosh-tui-provider-question-card\n".to_vec(),
                Duration::ZERO,
            ),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Agent question"), "{output}");
    assert!(!output.contains("ask_user_question called"), "{output}");
    assert!(
        !output.contains("Tool called: ask_user_question"),
        "{output}"
    );
    assert!(
        output.contains("Choose a color for cosh-tui provider follow-up"),
        "{output}"
    );
    assert!(output.contains("[1] Green"), "{output}");
    assert!(output.contains("[2] Blue"), "{output}");
    assert!(output.contains("Answer: Green"), "{output}");
    assert!(
        output.contains("Cosh-tui question answer received in same provider turn."),
        "{output}"
    );
    assert!(
        !output.contains("missing cosh-tui question answer"),
        "{output}"
    );
    assert!(
        !output.contains("question answer restarted provider instead of answering same turn"),
        "{output}"
    );
    assert!(!output.contains("Got your answer:"), "{output}");
    assert!(!output.contains("/answer"), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        !output.contains("bash: cosh-tui-provider-question-card: command not found"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_narrow_question_and_debug_remain_readable() {
    let home = temp_shell_home("cosh-tui-narrow-question-debug");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-narrow","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-narrow-question-debug*)
    printf '%s\n' '{"type":"control_request","request_id":"ask-cosh-tui-narrow-1","request":{"subtype":"ask_user","question":"Choose the narrow terminal follow-up action for cosh-tui provider output","options":[{"label":"Keep investigating"},{"label":"Open debug session"}],"allow_free_text":true,"multi_select":false}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ask-cosh-tui-narrow-1"'*'"answer":"Open debug session"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-narrow","message":{"content":[{"type":"text","text":"Cosh-tui narrow terminal answer received before debug."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-narrow","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-narrow","is_error":true,"result":"missing narrow question answer"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-narrow","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("TERM", "xterm-256color"),
            ("COSH_SHELL_WIDTH", "40"),
        ],
        vec![
            (
                b"?? cosh-tui-narrow-question-debug\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"Open debug session\n".to_vec(),
                Duration::from_millis(1_500),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    let compact = compact_terminal_words(&output);
    assert!(output.contains("Agent question"), "{output}");
    assert!(output.contains("Choose the narrow terminal"), "{output}");
    assert!(output.contains("cosh-tui provider output"), "{output}");
    assert!(output.contains("[1] Keep investigating"), "{output}");
    assert!(output.contains("[2] Open debug session"), "{output}");
    assert!(compact.contains("Answer: Open debug session"), "{output}");
    assert!(
        output.contains("Cosh-tui narrow terminal answer"),
        "{output}"
    );
    assert!(output.contains("received before debug."), "{output}");
    assert!(output.contains("provider invocation:"), "{output}");
    assert!(
        !output.contains("missing narrow question answer"),
        "{output}"
    );
    assert!(!output.contains("bash: /debug"), "{output}");
    assert!(
        !output.contains("bash: cosh-tui-narrow-question-debug: command not found"),
        "{output}"
    );
    assert_agent_block_width(&output, 40);
}

#[test]
fn raw_cli_cosh_tui_malformed_provider_event_failure_is_contained() {
    let home = temp_shell_home("cosh-tui-malformed-provider");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-malformed","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-malformed-provider-event*)
    printf '%s\n' '{"type":"assistant","session_id":'
    printf '%s\n' 'cosh-tui malformed provider fixture stderr' >&2
    exit 17
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-malformed","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (
                b"?? cosh-tui-malformed-provider-event\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"echo after-malformed-provider\nexit\n".to_vec(),
                Duration::from_millis(1_500),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("cosh-tui malformed provider fixture stderr"),
        "{output}"
    );
    assert!(output.contains("after-malformed-provider"), "{output}");
    assert!(
        !output.contains("bash: cosh-tui-malformed-provider-event: command not found"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_dumb_terminal_uses_plain_blocks() {
    let home = temp_shell_home("cosh-tui-dumb-terminal");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-dumb","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-dumb-terminal*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-dumb","message":{"content":[{"type":"text","text":"Cosh-tui dumb terminal response."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-dumb","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-dumb","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("NO_COLOR", "1"),
            ("TERM", "dumb"),
        ],
        vec![
            (b"?? cosh-tui-dumb-terminal\n".to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(2_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Cosh-tui dumb terminal response."),
        "{output}"
    );
    assert!(output.contains("Agent:"), "{output}");
    assert!(!output.contains("Agent status:"), "{output}");
    assert!(!output.contains('╭'), "{output}");
    assert!(!output.contains('│'), "{output}");
    assert!(!output.contains('╰'), "{output}");
}

#[test]
fn raw_cli_cosh_tui_cancel_then_exit_cleans_up_active_provider_process() {
    let home = temp_shell_home("cosh-tui-process-cleanup");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    let pid_file = home.join("cosh-tui.pid");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
printf '%s\n' "$$" > "$COSH_TUI_PID_FILE"
trap 'exit 0' TERM INT HUP
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-process-cleanup","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-process-cleanup*)
    sleep 60
    ;;
esac
sleep 60
"#,
    );

    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let pid_file_str = pid_file.to_string_lossy().to_string();
    let mut child = Command::new(binary)
        .args(["raw", "cosh-tui"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("HOME", &home_str)
        .env("COSH_TUI_PATH", &cosh_tui_path_str)
        .env("COSH_TUI_PID_FILE", &pid_file_str)
        .env("COSH_SHELL_ISOLATED", "1")
        .env("COSH_SHELL_RAW_SHELL", "bash")
        .env("COSH_SHELL_DEFAULT_SHELL", "bash")
        .env("COSH_SHELL_LANG", "en-US")
        .env("COSH_SHELL_BOOTSTRAP_PATH", "0")
        .process_group(0)
        .spawn()
        .expect("spawn cosh-shell raw");
    let raw_pid = child.id();
    let mut stdin = child.stdin.take().expect("child stdin");
    let writer = thread::spawn(move || {
        stdin
            .write_all(b"?? cosh-tui-process-cleanup\n")
            .expect("write prompt");
        stdin.flush().expect("flush prompt");
        thread::sleep(Duration::from_millis(1_200));
        stdin.write_all(b"/cancel\n").expect("write cancel");
        stdin.flush().expect("flush cancel");
        thread::sleep(Duration::from_millis(1_000));
        stdin.write_all(b"exit\n").expect("write exit");
        stdin.flush().expect("flush exit");
    });

    let status = child
        .wait_timeout(Duration::from_secs(10))
        .expect("wait raw process cleanup")
        .unwrap_or_else(|| {
            if let Ok(pid_text) = fs::read_to_string(&pid_file) {
                if let Ok(provider_pid) = pid_text.trim().parse::<u32>() {
                    signal_pid(provider_pid, "TERM");
                    thread::sleep(Duration::from_millis(100));
                    signal_pid(provider_pid, "KILL");
                }
            }
            signal_pid(raw_pid, "TERM");
            signal_process_group(raw_pid, "TERM");
            thread::sleep(Duration::from_millis(100));
            signal_pid(raw_pid, "KILL");
            signal_process_group(raw_pid, "KILL");
            panic!("cosh-shell raw did not exit after cancelling active provider")
        });
    writer.join().expect("join writer");
    let output = child.wait_with_output().expect("collect output");
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let provider_pid = read_pid_file_with_retry(&pid_file);

    for _ in 0..20 {
        if !process_is_alive(provider_pid) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let alive = process_is_alive(provider_pid);
    if alive {
        signal_pid(provider_pid, "TERM");
        thread::sleep(Duration::from_millis(100));
        signal_pid(provider_pid, "KILL");
    }
    let _ = fs::remove_dir_all(&home);

    assert!(status.success(), "status={status:?}\n{text}");
    assert!(text.contains("cosh-tui-process-cleanup"), "{text}");
    assert!(!alive, "provider pid {provider_pid} survived\n{text}");
}

#[test]
fn raw_cli_cosh_tui_completed_run_exit_leaves_no_provider_process() {
    let home = temp_shell_home("cosh-tui-process-cleanup-completed");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    let pid_file = home.join("cosh-tui.pid");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
printf '%s\n' "$$" > "$COSH_TUI_PID_FILE"
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-process-cleanup-completed","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-process-cleanup-completed*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-process-cleanup-completed","message":{"content":[{"type":"text","text":"Cosh-tui cleanup completed run."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-process-cleanup-completed","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-process-cleanup-completed","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let pid_file_str = pid_file.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_TUI_PID_FILE", &pid_file_str),
        ],
        vec![
            (
                b"?? cosh-tui-process-cleanup-completed\n".to_vec(),
                Duration::ZERO,
            ),
            (b"exit\n".to_vec(), Duration::from_millis(2_500)),
        ],
    );
    let provider_pid = read_pid_file_with_retry(&pid_file);
    let alive = process_is_alive(provider_pid);
    if alive {
        signal_pid(provider_pid, "TERM");
        thread::sleep(Duration::from_millis(100));
        signal_pid(provider_pid, "KILL");
    }
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Cosh-tui cleanup completed run."),
        "{output}"
    );
    assert!(!alive, "provider pid {provider_pid} survived\n{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_provider_disconnect_marks_recovery_reason() {
    let home = temp_shell_home("cosh-tui-host-executed-disconnect");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-disconnect","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-disconnect*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-disconnect","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-disconnect"}}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-disconnect","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-disconnect\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(
        output.contains("selected_shell_execution_path: foreground_shell_handoff_recovery"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: provider_run_not_active")
            || output.contains("provider_result_delivery_status: provider_channel_closed"),
        "{output}"
    );
    assert!(
        output.contains("recovery_reason: provider run was not active")
            || output.contains("recovery_reason: provider approval channel closed"),
        "{output}"
    );
    assert!(
        output.contains("latest recovery status: provider_run_not_active")
            || output.contains("latest recovery status: provider_channel_closed"),
        "{output}"
    );
    assert!(
        output.contains("latest recovery reason: provider run was not active")
            || output.contains("latest recovery reason: provider approval channel closed"),
        "{output}"
    );
    assert!(
        output.contains("latest provider request: ctrl-cosh-tui-disconnect"),
        "{output}"
    );
    assert!(
        output.contains("latest tool use id: toolu-cosh-tui-disconnect"),
        "{output}"
    );
    assert!(
        !output.contains("control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        !output.contains("bash: cosh-tui-provider-host-executed-disconnect: command not found"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_tui_duplicate_host_executed_shell_request_is_not_executed_twice() {
    let home = temp_shell_home("cosh-tui-host-executed-duplicate");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-duplicate","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-duplicate*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-dup","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-dup"}}'
    IFS= read -r response1 || exit 2
    case "$response1" in
      *'"behavior":"host_executed_shell"'*'df -h'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":true,"result":"missing first host result"}'; exit 1 ;;
    esac
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-dup","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-dup"}}'
    IFS= read -r response2 || exit 2
    case "$response2" in
      *'"behavior":"deny"'*'Duplicate shell tool request was already completed'*)
        printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-duplicate","message":{"content":[{"type":"text","text":"Duplicate host-executed shell request denied without second execution."}]}}'
        printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":false,"result":"done"}'
        exit 0
        ;;
    esac
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":true,"result":"duplicate request was not denied"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-duplicate","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-duplicate\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(7_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("Duplicate host-executed shell request denied without second execution."),
        "{output}"
    );
    assert!(
        !output.contains("duplicate request was not denied"),
        "{output}"
    );
    assert_eq!(count_occurrences(&normalized, "\ndf -h\n"), 1, "{output}");
    assert_eq!(count_occurrences(&normalized, "Filesystem"), 1, "{output}");
    assert!(
        output.contains("host-executed shell result: delivered"),
        "{output}"
    );
    assert!(
        output.contains("latest provider request: ctrl-cosh-tui-dup"),
        "{output}"
    );
    assert!(
        output.contains("latest tool use id: toolu-cosh-tui-dup"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_nonzero_returns_normal_tool_result() {
    let home = temp_shell_home("cosh-tui-host-executed-nonzero");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-nonzero","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-nonzero*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-nonzero","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"false"},"tool_use_id":"toolu-cosh-tui-nonzero"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'"exit_code":1'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-nonzero","message":{"content":[{"type":"text","text":"Cosh-tui nonzero host-executed result received as normal tool result."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-nonzero","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-nonzero","is_error":true,"result":"missing nonzero host result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-nonzero","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-nonzero\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"true\n".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ false"), "{output}");
    assert!(output.contains("Shell: failed · req-1"), "{output}");
    assert!(
        output.contains("Cosh-tui nonzero host-executed result received as normal tool result."),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("status: failed"), "{output}");
    assert!(output.contains("exit_code: 1"), "{output}");
    assert!(!output.contains("missing nonzero host result"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("The command false failed"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_long_command_continues_same_turn() {
    let home = temp_shell_home("cosh-tui-host-executed-long");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-long","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-long*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-long","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"sleep 1; echo hostexec-done"},"tool_use_id":"toolu-cosh-tui-long"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'sleep 1; echo hostexec-done'*'hostexec-done'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-long","message":{"content":[{"type":"text","text":"Cosh-tui long host-executed command continued in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-long","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-long","is_error":true,"result":"missing long host result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-long","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-long\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(7_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ sleep 1; echo hostexec-done"), "{output}");
    assert!(output.contains("hostexec-done"), "{output}");
    assert!(
        output.contains("Cosh-tui long host-executed command continued in same provider turn."),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(!output.contains("missing long host result"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_host_executed_large_output_is_bounded() {
    let home = temp_shell_home("cosh-tui-host-executed-large");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-large","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-large*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-large","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"printf %08000d 0"},"tool_use_id":"toolu-cosh-tui-large"}}'
    if IFS= read -r response; then
      response_len=${#response}
      case "$response" in
        *'"behavior":"host_executed_shell"'*'bounded_output_summary'*)
          if [ "$response_len" -gt 7000 ]; then
            printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-large","is_error":true,"result":"host result was not bounded"}'
            exit 1
          fi
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-large","message":{"content":[{"type":"text","text":"Cosh-tui large host-executed output was bounded for provider."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-large","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-large","is_error":true,"result":"missing bounded large host result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-large","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-large\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(7_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ printf %08000d 0"), "{output}");
    assert!(
        output.contains("Cosh-tui large host-executed output was bounded for provider."),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(
        !output.contains("missing bounded large host result"),
        "{output}"
    );
    assert!(!output.contains("host result was not bounded"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
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

#[test]
fn raw_cli_manual_approval_host_executed_shell_result_continues_same_turn() {
    let home = temp_shell_home("qwen-manual-host-executed-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-manual-host-executed","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *manual-provider-host-executed-shell*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-manual","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"sudo -V"},"tool_use_id":"toolu-manual"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*bounded_output_summary*'sudo -V'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-manual-host-executed","message":{"content":[{"type":"text","text":"Manual host-executed shell result received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-manual-host-executed","is_error":false,"result":"done"}'
          exit 0
          ;;
        *'"behavior":"allow"'*)
          printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-manual-host-executed","is_error":true,"result":"unexpected provider-native allow"}'
          exit 1
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-manual-host-executed","is_error":true,"result":"missing manual host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-manual-host-executed","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? manual-provider-host-executed-shell\n".to_vec(),
                Duration::from_millis(300),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ sudo -V"), "{output}");
    assert!(
        output.contains("Manual host-executed shell result received in same provider turn."),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(
        !output.contains("unexpected provider-native allow"),
        "{output}"
    );
    assert!(
        !output.contains("missing manual host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_host_executed_nonzero_exit_returns_normal_tool_result() {
    let home = temp_shell_home("qwen-host-executed-nonzero");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-nonzero","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-nonzero*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-nonzero","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"false"},"tool_use_id":"toolu-nonzero"}}'
    if IFS= read -r response; then
      case "$response" in
        *host_executed_shell*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-nonzero","message":{"content":[{"type":"text","text":"Host-executed nonzero result received as normal tool result."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-nonzero","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-nonzero","is_error":true,"result":"missing nonzero host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-nonzero","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider-host-executed-nonzero\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"true\nexit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ false"), "{output}");
    assert!(output.contains("Shell: failed · req-1"), "{output}");
    assert!(
        output.contains("Host-executed nonzero result received as normal tool result."),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("status: failed"), "{output}");
    assert!(output.contains("exit_code: 1"), "{output}");
    assert!(
        !output.contains("missing nonzero host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("The command false failed"), "{output}");
}

#[test]
fn raw_cli_host_executed_interrupt_returns_normal_tool_result() {
    let home = temp_shell_home("qwen-host-executed-interrupt");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-interrupt","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-interrupt*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-interrupt","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"sh -c '\''exit 130'\''"},"tool_use_id":"toolu-interrupt"}}'
    if IFS= read -r response; then
      case "$response" in
        *host_executed_shell*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-interrupt","message":{"content":[{"type":"text","text":"Host-executed interrupt result received as normal tool result."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-interrupt","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-interrupt","is_error":true,"result":"missing interrupt host_executed_shell result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-interrupt","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider-host-executed-interrupt\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"true\nexit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ sh -c 'exit 130'"), "{output}");
    assert!(output.contains("Shell: interrupted · req-1"), "{output}");
    assert!(
        output.contains("Host-executed interrupt result received as normal tool result."),
        "{output}"
    );
    assert!(
        output
            .contains("selected_shell_execution_path: control_protocol_host_executed_shell_result"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: delivered"),
        "{output}"
    );
    assert!(output.contains("status: interrupted"), "{output}");
    assert!(output.contains("exit_code: 130"), "{output}");
    assert!(
        !output.contains("missing interrupt host_executed_shell result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(
        !output.contains("The command sh -c 'exit 130' failed"),
        "{output}"
    );
}

#[test]
fn raw_cli_host_executed_multi_tool_keeps_single_turn_boundary() {
    let home = temp_shell_home("qwen-host-executed-multi-tool");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-multi","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-multi","message":{"content":[{"type":"text","text":"UNEXPECTED FRESH CONTINUATION"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-multi","is_error":false,"result":"unexpected"}'
    exit 0
    ;;
esac
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-multi","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-multi-tool*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
    IFS= read -r response1 || exit 2
    case "$response1" in
      *'"behavior":"host_executed_shell"'*'df -h'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-multi","is_error":true,"result":"missing first host result"}'; exit 1 ;;
    esac
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-multi","message":{"content":[{"type":"text","text":"FIRST TOOL ANALYSIS TEXT"}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-2","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"du -sh ."},"tool_use_id":"toolu-2"}}'
    IFS= read -r response2 || exit 2
    case "$response2" in
      *'"behavior":"host_executed_shell"'*'du -sh .'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-multi","is_error":true,"result":"missing second host result"}'; exit 1 ;;
    esac
    sleep 2
    printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-multi","message":{"content":[{"type":"text","text":"FINAL MULTI TOOL REPORT"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-multi","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-multi","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-host-executed-multi-tool\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"echo AFTER_PROVIDER_INPUT\n".to_vec(),
                Duration::from_millis(1_500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(3_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Auto-approved req-2"), "{output}");
    assert!(output.contains("FIRST TOOL ANALYSIS TEXT"), "{output}");
    assert!(output.contains("FINAL MULTI TOOL REPORT"), "{output}");
    assert!(!output.contains("missing first host result"), "{output}");
    assert!(!output.contains("missing second host result"), "{output}");
    assert!(
        !output.contains("UNEXPECTED FRESH CONTINUATION"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("Agent 恢复"), "{output}");
    assert!(!output.contains("Using a fresh provider turn"), "{output}");
    assert!(!output.contains("Shell recovery"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert_eq!(
        count_occurrences_between(
            &normalized,
            "cosh-osc$ du -sh .\n",
            "FINAL MULTI TOOL REPORT",
            "cosh-osc$"
        ),
        0,
        "{output}"
    );
    assert_eq!(
        count_occurrences_between(
            &normalized,
            "cosh-osc$ du -sh .\n",
            "FINAL MULTI TOOL REPORT",
            "Thinking..."
        ),
        0,
        "{output}"
    );
    assert!(
        !normalized.contains("cosh-osc$ cosh-osc$ echo AFTER_PROVIDER_INPUT"),
        "{output}"
    );
    assert_inline_before_followup(
        &normalized,
        "FINAL MULTI TOOL REPORT",
        "AFTER_PROVIDER_INPUT",
    );
}

#[test]
fn raw_cli_cosh_tui_host_executed_multi_tool_keeps_single_turn_boundary() {
    let home = temp_shell_home("cosh-tui-host-executed-multi-tool");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-host-executed-multi","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-host-executed-multi-tool*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-multi-1","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-multi-1"}}'
    IFS= read -r response1 || exit 2
    case "$response1" in
      *'"behavior":"host_executed_shell"'*'df -h'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-multi","is_error":true,"result":"missing first cosh-tui host result"}'; exit 1 ;;
    esac
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-multi","message":{"content":[{"type":"text","text":"FIRST COSH-TUI TOOL ANALYSIS TEXT"}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-multi-2","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"du -sh ."},"tool_use_id":"toolu-cosh-tui-multi-2"}}'
    IFS= read -r response2 || exit 2
    case "$response2" in
      *'"behavior":"host_executed_shell"'*'du -sh .'*) ;;
      *) printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-host-executed-multi","is_error":true,"result":"missing second cosh-tui host result"}'; exit 1 ;;
    esac
    sleep 2
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-host-executed-multi","message":{"content":[{"type":"text","text":"FINAL COSH-TUI MULTI TOOL REPORT"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-multi","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-host-executed-multi","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-host-executed-multi-tool\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"echo AFTER_COSH_TUI_PROVIDER_INPUT\n".to_vec(),
                Duration::from_millis(1_500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(3_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    let normalized = output.replace('\r', "");

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Auto-approved req-2"), "{output}");
    assert!(
        output.contains("FIRST COSH-TUI TOOL ANALYSIS TEXT"),
        "{output}"
    );
    assert!(
        output.contains("FINAL COSH-TUI MULTI TOOL REPORT"),
        "{output}"
    );
    assert!(
        !output.contains("missing first cosh-tui host result"),
        "{output}"
    );
    assert!(
        !output.contains("missing second cosh-tui host result"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("Agent 恢复"), "{output}");
    assert!(!output.contains("Using a fresh provider turn"), "{output}");
    assert!(!output.contains("Shell recovery"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert_eq!(
        count_occurrences_between(
            &normalized,
            "cosh-osc$ du -sh .\n",
            "FINAL COSH-TUI MULTI TOOL REPORT",
            "cosh-osc$"
        ),
        0,
        "{output}"
    );
    assert_inline_before_followup(
        &normalized,
        "FINAL COSH-TUI MULTI TOOL REPORT",
        "AFTER_COSH_TUI_PROVIDER_INPUT",
    );
}

#[test]
fn raw_cli_streamed_tool_fallback_with_host_capability_is_not_delivered() {
    let home = temp_shell_home("qwen-streamed-tool-fallback-no-delivery");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback","message":{"content":[{"type":"text","text":"STREAMED FALLBACK RECOVERY ONLY"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *streamed-tool-fallback-no-delivery*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_fallback","name":"run_shell_command","input":{"command":"echo STREAMED_FALLBACK"}}]}}'
    sleep 30
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"streamed-tool-fallback-no-delivery\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(5_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ echo STREAMED_FALLBACK"), "{output}");
    assert!(output.contains("STREAMED_FALLBACK"), "{output}");
    assert!(
        output.contains("STREAMED FALLBACK RECOVERY ONLY"),
        "{output}"
    );
    assert!(
        output.contains("selected_shell_execution_path: foreground_shell_handoff_recovery"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: not_provider_tool_request"),
        "{output}"
    );
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(
        !output.contains("control_protocol_host_executed_shell_result"),
        "{output}"
    );
}

#[test]
fn raw_cli_streamed_tool_fallback_recovery_blocks_new_shell_tool() {
    let home = temp_shell_home("qwen-streamed-tool-fallback-recovery-blocks-shell");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback-block","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback-block","message":{"id":"m2","type":"message","role":"assistant","model":"qwen","content":[{"type":"text","text":"RECOVERY TRIED SECOND SHELL TOOL"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback-block","message":{"id":"m3","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_recovery","name":"run_shell_command","input":{"command":"echo SHOULD_NOT_RUN_IN_RECOVERY"}}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback-block","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-streamed-fallback-block","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *streamed-tool-fallback-blocks-recovery-tool*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-streamed-fallback-block","message":{"id":"m1","type":"message","role":"assistant","model":"qwen","content":[{"type":"tool_use","id":"call_fallback","name":"run_shell_command","input":{"command":"echo STREAMED_ONCE"}}]}}'
    sleep 30
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-streamed-fallback-block","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"streamed-tool-fallback-blocks-recovery-tool\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(5_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ echo STREAMED_ONCE"), "{output}");
    assert!(output.contains("STREAMED_ONCE"), "{output}");
    assert!(
        output.contains("RECOVERY TRIED SECOND SHELL TOOL"),
        "{output}"
    );
    assert_eq!(
        count_occurrences(&output, "Bash tool sent to shell"),
        1,
        "{output}"
    );
    assert!(
        !output.contains("cosh-osc$ echo SHOULD_NOT_RUN_IN_RECOVERY"),
        "{output}"
    );
    assert!(!output.contains("Auto-approved req-2"), "{output}");
    assert!(
        output.contains("provider_result_delivery_status: not_provider_tool_request"),
        "{output}"
    );
}

#[test]
fn raw_cli_qwen_streamed_non_shell_tool_renders_activity_card() {
    let home = temp_shell_home("qwen-streamed-non-shell-tool-activity");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-tool-activity","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *streamed-non-shell-tool-activity*)
    printf '%s\n' '{"type":"stream_event","session_id":"sess-tool-activity","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_read","name":"Read","input":{}}}}'
    printf '%s\n' '{"type":"stream_event","session_id":"sess-tool-activity","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"Cargo.toml\"}"}}}'
    printf '%s\n' '{"type":"stream_event","session_id":"sess-tool-activity","event":{"type":"content_block_stop","index":0}}'
    printf '%s\n' '{"type":"user","session_id":"sess-tool-activity","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_read","is_error":false,"content":"read output"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-tool-activity","message":{"role":"assistant","content":[{"type":"text","text":"READ VISIBILITY FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-tool-activity","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-tool-activity","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? streamed-non-shell-tool-activity\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(1_500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Read called: Cargo.toml; [Details] tool-1"),
        "{output}"
    );
    assert!(output.contains("Activity details tool-1"), "{output}");
    assert!(output.contains("evidence: ProviderToolCall"), "{output}");
    assert!(
        output.contains("provider: provider_native_stream"),
        "{output}"
    );
    assert!(output.contains("tool_name: Read"), "{output}");
    assert!(output.contains("input_preview: Cargo.toml"), "{output}");
    assert!(output.contains("READ VISIBILITY FINAL"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_provider_native_tool_results_are_visible() {
    let home = temp_shell_home("cosh-tui-provider-native-tool-visible");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-native-visible","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-native-visible*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-visible","message":{"content":[{"type":"tool_use","id":"call_cosh_tui_shell","name":"shell","input":{"command":"echo COSH_TUI_NATIVE_SHELL"}}]}}'
    printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-native-visible","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_cosh_tui_shell","is_error":false,"content":"COSH_TUI_NATIVE_SHELL\n"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-visible","message":{"content":[{"type":"text","text":"COSH TUI PROVIDER NATIVE VISIBLE FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-visible","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-visible","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-provider-native-visible\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(1_500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("$ echo COSH_TUI_NATIVE_SHELL"), "{output}");
    assert!(output.contains("COSH_TUI_NATIVE_SHELL"), "{output}");
    assert!(
        output.contains("auto-approved by provider: $ echo COSH_TUI_NATIVE_SHELL; [Details]"),
        "{output}"
    );
    assert!(
        output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
    assert!(
        output.contains("COSH TUI PROVIDER NATIVE VISIBLE FINAL"),
        "{output}"
    );
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("missing host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_provider_native_shell_omits_duplicate_activity_in_normal_ui() {
    let home = temp_shell_home("cosh-tui-provider-native-shell-no-activity");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-native-no-activity","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-native-no-activity*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-no-activity","message":{"content":[{"type":"tool_use","id":"call_shell_no_activity","name":"shell","input":{"command":"echo COSH_TUI_NATIVE_NO_ACTIVITY"}}]}}'
    printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-native-no-activity","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_shell_no_activity","is_error":false,"content":"COSH_TUI_NATIVE_NO_ACTIVITY\n"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-no-activity","message":{"content":[{"type":"text","text":"COSH TUI PROVIDER NATIVE NO ACTIVITY FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-no-activity","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-no-activity","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-native-no-activity\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("$ echo COSH_TUI_NATIVE_NO_ACTIVITY"),
        "{output}"
    );
    assert!(output.contains("COSH_TUI_NATIVE_NO_ACTIVITY"), "{output}");
    assert!(
        output.contains("COSH TUI PROVIDER NATIVE NO ACTIVITY FINAL"),
        "{output}"
    );
    assert!(!output.contains("auto-approved by provider"), "{output}");
    assert!(
        !output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_tui_control_request_suppresses_matching_shell_snapshot() {
    let home = temp_shell_home("cosh-tui-control-suppresses-shell-snapshot");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-control-snapshot","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-control-snapshot-dup*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-control-snapshot","message":{"content":[{"type":"tool_use","id":"toolu-cosh-tui-dup","name":"shell","input":{"command":"df -h"}}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-cosh-tui-dup","request":{"subtype":"can_use_tool","tool_name":"shell","input":{"command":"df -h"},"tool_use_id":"toolu-cosh-tui-dup"}}'
    IFS= read -r response || exit 2
    case "$response" in
      *'"behavior":"host_executed_shell"'*'df -h'*)
        printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-control-snapshot","message":{"content":[{"type":"text","text":"COSH TUI CONTROL SNAPSHOT FINAL"}]}}'
        printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-control-snapshot","is_error":false,"result":"done"}'
        exit 0
        ;;
    esac
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-control-snapshot","is_error":true,"result":"missing host executed result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-control-snapshot","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-control-snapshot-dup\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(2_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        output.contains("COSH TUI CONTROL SNAPSHOT FINAL"),
        "{output}"
    );
    assert!(!output.contains("auto-approved by provider"), "{output}");
    assert!(
        !output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
    assert!(!output.contains("missing host executed result"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_streamed_provider_native_shell_result_renders_before_final_text() {
    let home = temp_shell_home("cosh-tui-streamed-provider-native-shell-visible");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-streamed-native-visible","model":"cosh-tui-test"}'
read -r user_message
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"message_start"}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"COSH TUI PRE TOOL TEXT STREAMS"}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_cosh_tui_stream_shell","name":"shell","input":{}}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"echo COSH_TUI_STREAM_NATIVE_SHELL\"}"}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_stop","index":0}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"COSH TUI POST TOOL TEXT SHOULD WAIT"}}}'
printf '%s\n' '{"type":"stream_event","session_id":"sess-cosh-tui-streamed-native-visible","event":{"type":"message_stop"}}'
sleep 1
printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-streamed-native-visible","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_cosh_tui_stream_shell","is_error":false,"content":"COSH_TUI_STREAM_NATIVE_SHELL\n"}]}}'
printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-streamed-native-visible","message":{"content":[{"type":"text","text":"COSH TUI STREAM PROVIDER NATIVE FINAL"}]}}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-streamed-native-visible","is_error":false,"result":"done"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("COSH_SHELL_DEBUG", "1"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? cosh-tui-streamed-provider-native-visible\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(3_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    let command_pos = output
        .find("$ echo COSH_TUI_STREAM_NATIVE_SHELL")
        .unwrap_or_else(|| panic!("{output}"));
    let stdout_pos = output
        .find("COSH_TUI_STREAM_NATIVE_SHELL")
        .unwrap_or_else(|| panic!("{output}"));
    let pre_tool_text_pos = output
        .find("COSH TUI PRE TOOL TEXT STREAMS")
        .unwrap_or_else(|| panic!("{output}"));
    let post_tool_text_pos = output
        .find("COSH TUI POST TOOL TEXT SHOULD WAIT")
        .unwrap_or_else(|| panic!("{output}"));
    let final_pos = output
        .find("COSH TUI STREAM PROVIDER NATIVE FINAL")
        .unwrap_or_else(|| panic!("{output}"));
    assert!(pre_tool_text_pos < command_pos, "{output}");
    assert!(command_pos < post_tool_text_pos, "{output}");
    assert!(command_pos < final_pos, "{output}");
    assert!(stdout_pos < post_tool_text_pos, "{output}");
    assert!(stdout_pos < final_pos, "{output}");
    assert!(
        output
            .contains("auto-approved by provider: $ echo COSH_TUI_STREAM_NATIVE_SHELL; [Details]"),
        "{output}"
    );
    assert!(
        output.contains("provider_native_shell_bypassed_control_protocol"),
        "{output}"
    );
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_provider_native_non_shell_result_is_visible() {
    let home = temp_shell_home("cosh-tui-provider-native-non-shell-visible");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-native-non-shell-visible","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-native-non-shell-visible*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-non-shell-visible","message":{"content":[{"type":"tool_use","id":"call_cosh_tui_read","name":"Read","input":{"file_path":"Cargo.toml"}}]}}'
    printf '%s\n' '{"type":"user","session_id":"sess-cosh-tui-native-non-shell-visible","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_cosh_tui_read","is_error":false,"content":"read output visible"}]}}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-native-non-shell-visible","message":{"content":[{"type":"text","text":"COSH TUI PROVIDER NATIVE NON SHELL FINAL"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-non-shell-visible","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-native-non-shell-visible","is_error":false,"result":"ignored"}'
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
                b"?? cosh-tui-provider-native-non-shell-visible\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"/details tool-1\n".to_vec(), Duration::from_millis(1_500)),
            (b"/details out-1\n".to_vec(), Duration::from_millis(500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(
        output.contains("Read called: Cargo.toml; [Details]"),
        "{output}"
    );
    assert!(output.contains("evidence: ProviderToolCall"), "{output}");
    assert!(output.contains("input_preview: Cargo.toml"), "{output}");
    assert!(output.contains("Tool output - stdout captured"), "{output}");
    assert!(output.contains("read output visible"), "{output}");
    assert!(
        output.contains("COSH TUI PROVIDER NATIVE NON SHELL FINAL"),
        "{output}"
    );
}

#[test]
fn raw_cli_host_executed_provider_disconnect_marks_recovery_reason() {
    let home = temp_shell_home("qwen-host-executed-disconnect");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-disconnect","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-disconnect*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-disconnect","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-host-executed-disconnect\n".to_vec(),
                Duration::from_millis(500),
            ),
            (
                b"/details handoff-1\n".to_vec(),
                Duration::from_millis(6_000),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(
        output.contains("selected_shell_execution_path: foreground_shell_handoff_recovery"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: provider_run_not_active")
            || output.contains("provider_result_delivery_status: provider_channel_closed"),
        "{output}"
    );
    assert!(
        output.contains("recovery_reason: provider run was not active")
            || output.contains("recovery_reason: provider approval channel closed"),
        "{output}"
    );
    assert!(
        output.contains("latest recovery status: provider_run_not_active")
            || output.contains("latest recovery status: provider_channel_closed"),
        "{output}"
    );
    assert!(
        output.contains("latest recovery reason: provider run was not active")
            || output.contains("latest recovery reason: provider approval channel closed"),
        "{output}"
    );
    assert!(
        !output.contains("control_protocol_host_executed_shell_result"),
        "{output}"
    );
}

#[test]
fn raw_cli_host_executed_shell_timeout_interrupts_and_returns_result() {
    let output = run_host_executed_shell_timeout(&[]);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ sleep 10"), "{output}");
    assert!(
        output.contains("Command exceeded configured shell handoff timeout (1s)."),
        "{output}"
    );
    assert!(
        output.contains("Sent interrupt to foreground PTY; waiting for shell evidence."),
        "{output}"
    );
    assert!(
        output.contains("Host-executed timeout interrupt result received."),
        "{output}"
    );
    assert!(output.contains("Shell: timed_out · req-1"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("req-2"), "{output}");
    assert!(
        !output.contains("missing timeout interrupt result"),
        "{output}"
    );
}

#[test]
fn raw_cli_host_executed_shell_timeout_uses_zh_language_env() {
    let output = run_host_executed_shell_timeout(&[("COSH_SHELL_LANG", "zh-CN")]);

    assert!(output.contains("已批准 req-1"), "{output}");
    assert!(output.contains("Bash tool 已发送到 shell"), "{output}");
    assert!(output.contains("$ sleep 10"), "{output}");
    assert!(
        output.contains("命令超过了配置的 shell handoff 超时时间（1s）。"),
        "{output}"
    );
    assert!(
        output.contains("已向前台 PTY 发送中断；正在等待 shell evidence。"),
        "{output}"
    );
    assert!(
        output.contains("Host-executed timeout interrupt result received."),
        "{output}"
    );
    assert!(output.contains("Shell: timed_out · req-1"), "{output}");
    assert!(!output.contains("req-2"), "{output}");
    assert!(!output.contains("Shell recovery"), "{output}");
    assert!(
        !output.contains("Command exceeded configured shell handoff timeout"),
        "{output}"
    );
    assert!(
        !output.contains("Sent interrupt to foreground PTY"),
        "{output}"
    );
    assert!(
        !output.contains("missing timeout interrupt result"),
        "{output}"
    );
}

fn run_host_executed_shell_timeout(extra_env: &[(&str, &str)]) -> String {
    let home = temp_shell_home("qwen-host-executed-timeout");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-host-executed-timeout","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-host-executed-timeout*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-timeout","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"sleep 10"},"tool_use_id":"toolu-timeout"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"behavior":"host_executed_shell"'*'sleep 10'*'"status":"timed_out"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-host-executed-timeout","message":{"content":[{"type":"text","text":"Host-executed timeout interrupt result received."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-timeout","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-host-executed-timeout","is_error":true,"result":"missing timeout interrupt result"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-host-executed-timeout","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let mut env = vec![
        ("HOME", home_str.as_str()),
        ("PATH", path.as_str()),
        ("COSH_SHELL_HANDOFF_TIMEOUT_SECS", "1"),
    ];
    env.extend_from_slice(extra_env);
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &env,
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-host-executed-timeout\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (b"exit 0\n".to_vec(), Duration::from_millis(5_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);
    output
}

#[test]
fn raw_cli_non_shell_permission_passes_through_allow_only() {
    let home = temp_shell_home("qwen-non-shell-pass-through");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-non-shell-pass-through","model":"qwen-test"}'
read -r user_message
case "$user_message" in
  *provider-read-pass-through*)
    printf '%s\n' '{"type":"control_request","request_id":"ctrl-read","request":{"subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"README.md"},"tool_use_id":"toolu-read"}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ctrl-read"'*'"behavior":"allow"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-non-shell-pass-through","message":{"content":[{"type":"text","text":"Read permission allowed through provider control protocol."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-non-shell-pass-through","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-non-shell-pass-through","is_error":true,"result":"missing non-shell allow response"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-non-shell-pass-through","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-read-pass-through\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(3_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(
        output.contains("Read permission allowed through provider control protocol."),
        "{output}"
    );
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(!output.contains("host_executed_shell"), "{output}");
    assert!(!output.contains("foreground_shell_pty"), "{output}");
    assert!(
        !output.contains("missing non-shell allow response"),
        "{output}"
    );
}

#[test]
fn raw_cli_obvious_tty_provider_shell_permission_uses_foreground_recovery() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider tty shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(2_000)),
            (
                b"/details handoff-1\nexit 0\n".to_vec(),
                Duration::from_millis(4_000),
            ),
        ],
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("medium risk"), "{output}");
    assert!(!output.contains("high risk"), "{output}");
    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ ssh -V"), "{output}");
    assert!(
        output.contains("execution_path: foreground_shell_pty"),
        "{output}"
    );
    assert!(
        output.contains("selected_shell_execution_path: foreground_shell_handoff_recovery"),
        "{output}"
    );
    assert!(
        output.contains("provider_result_delivery_status: provider_run_not_active"),
        "{output}"
    );
    assert!(
        output.contains("recovery_reason: provider run was not active when shell completed"),
        "{output}"
    );
    assert!(output.contains("output_id: terminal-output://"), "{output}");
    assert!(!output.contains("output_ref:"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert!(
        !output.contains("PROVIDER TTY OUTPUT SHOULD NOT RENDER AFTER RECOVERY"),
        "{output}"
    );
}

#[test]
fn raw_cli_debug_mode_keeps_control_shell_output_foreground_owned() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_DEBUG", "1")],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? provider auto safe shell\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );

    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(
        output.contains("Tool requested: Bash requested: $ df -h"),
        "{output}"
    );
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        !output.contains("PROVIDER AUTO NATIVE OUTPUT RENDERED AFTER ALLOW"),
        "{output}"
    );
    assert!(!output.contains("Tool output: stdout captured"), "{output}");
}

#[test]
fn raw_cli_tool_output_does_not_break_markdown_stream_finalization() {
    let output = run_raw_cli_with_input("fake", "?? tool output finalization\nexit\n");

    assert!(output.contains("Before tool"), "{output}");
    assert!(output.contains("After tool"), "{output}");
    assert_ordered(&output, &["Before tool", "After tool"]);
    assert!(!output.contains("Tool output:"), "{output}");
    assert!(!output.contains("Tool completed"), "{output}");
    assert!(!output.contains("Governance:"), "{output}");
    assert!(
        !output.contains("bash: ?? tool output finalization"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_request_history_auto_sends_history_index() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"echo before-history\n".to_vec(), Duration::ZERO),
            (
                b"?? request shell history evidence\n".to_vec(),
                Duration::from_millis(300),
            ),
            (
                b"/details cosh-request-1\n".to_vec(),
                Duration::from_millis(600),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(600)),
        ],
    );

    assert!(output.contains("before-history"), "{output}");
    assert!(!output.contains("Agent Requested Evidence"), "{output}");
    assert!(
        output.contains("Evidence history index received by fake adapter."),
        "{output}"
    );
    assert!(output.contains("cosh-request details"), "{output}");
    assert!(output.contains("request_id: cosh-request-1"), "{output}");
    assert!(output.contains("outcome: parsed"), "{output}");
    assert!(output.contains("reason: parsed"), "{output}");
    assert!(output.contains("raw_block:"), "{output}");
    assert!(
        compact_terminal_words(&output).contains("```cosh-requesthistory```"),
        "{output}"
    );
    assert!(
        !output.contains("bash: request shell history evidence"),
        "{output}"
    );
}

#[test]
fn raw_cli_cosh_request_history_redaction_requires_confirmation() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"echo token=super-secret\n".to_vec(), Duration::ZERO),
            (
                b"?? request shell history evidence\n".to_vec(),
                Duration::from_millis(300),
            ),
            (b"\n".to_vec(), Duration::from_millis(800)),
            (b"exit\n".to_vec(), Duration::from_millis(600)),
        ],
    );

    assert!(output.contains("Agent Requested Evidence"), "{output}");
    assert!(
        output.contains("Agent wants to inspect the recent shell command index."),
        "{output}"
    );
    assert!(
        output.contains("Redacted history index received by fake adapter."),
        "{output}"
    );
    assert!(output.contains("token=super-secret"), "{output}");
    assert!(!output.contains("bounded_output_excerpt:"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
}

#[test]
fn raw_cli_cosh_request_output_card_sends_bounded_excerpt() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"printf 'alpha\\nbeta\\ngamma\\n'\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"?? request captured output evidence\n".to_vec(),
                Duration::from_millis(300),
            ),
            (b"\n".to_vec(), Duration::from_millis(800)),
            (b"exit\n".to_vec(), Duration::from_millis(600)),
        ],
    );

    assert!(output.contains("alpha"), "{output}");
    assert!(output.contains("beta"), "{output}");
    assert!(output.contains("gamma"), "{output}");
    assert!(output.contains("Agent Requested Evidence"), "{output}");
    assert!(
        output.contains(
            "Agent wants to inspect captured output: terminal-output://raw-session/cmd-1 tail"
        ),
        "{output}"
    );
    assert!(output.contains("Max lines: 2"), "{output}");
    assert!(
        output.contains("Evidence excerpt received by fake adapter: beta gamma"),
        "{output}"
    );
    assert!(!output.contains("```cosh-request"), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
    assert!(!output.contains("cosh-osc$ cosh-osc$"), "{output}");
}

#[test]
fn raw_cli_cosh_request_card_ignore_keeps_shell_usable() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"printf 'alpha\\nbeta\\ngamma\\n'\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"?? request captured output evidence\n".to_vec(),
                Duration::from_millis(300),
            ),
            (b"i\n".to_vec(), Duration::from_millis(1_500)),
            (
                b"echo after-evidence-ignore\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert!(output.contains("Agent Requested Evidence"), "{output}");
    assert!(
        output.contains("Ignored this evidence request."),
        "{output}"
    );
    assert!(output.contains("after-evidence-ignore"), "{output}");
    assert!(
        !output.contains("Evidence excerpt received by fake adapter:"),
        "{output}"
    );
    assert!(!output.contains("bash: i"), "{output}");
    assert!(!output.contains("cosh-osc$ cosh-osc$"), "{output}");
}

#[test]
fn raw_cli_cosh_request_card_ctrl_c_cancels_only_evidence_request() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"printf 'alpha\\nbeta\\ngamma\\n'\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"?? request captured output evidence\n".to_vec(),
                Duration::from_millis(300),
            ),
            (vec![0x03], Duration::from_millis(1_500)),
            (
                b"echo after-evidence-cancel\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert!(output.contains("Agent Requested Evidence"), "{output}");
    assert!(output.contains("after-evidence-cancel"), "{output}");
    assert!(
        !output.contains("Evidence excerpt received by fake adapter:"),
        "{output}"
    );
    assert!(!output.contains("Agent cancellation requested"), "{output}");
    assert!(
        !output.contains("Reason: user requested cancellation"),
        "{output}"
    );
    assert!(!output.contains("cosh-osc$ cosh-osc$"), "{output}");
}

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
            "Skill failed: recovery-context",
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
fn raw_cli_shell_handoff_continuation_denies_second_shell_tool() {
    let output = run_qwen_continuation_deny("qwen-continuation-deny", &[]);

    assert!(output.contains("Approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ ssh -V"), "{output}");
    assert!(
        output.contains("Continuation summarized existing shell evidence in plan mode."),
        "{output}"
    );
    assert!(!output.contains("$ du -sh ~"), "{output}");
    assert_eq!(
        count_occurrences(&output, "Approval required"),
        1,
        "{output}"
    );
}

#[test]
fn raw_cli_zh_shell_handoff_continuation_denies_second_shell_tool() {
    let output =
        run_qwen_continuation_deny("qwen-continuation-deny-zh", &[("COSH_SHELL_LANG", "zh-CN")]);

    assert!(output.contains("已批准 req-1"), "{output}");
    assert!(output.contains("Bash tool 已发送到 shell"), "{output}");
    assert!(output.contains("$ ssh -V"), "{output}");
    assert!(
        output.contains("Continuation summarized existing shell evidence in plan mode."),
        "{output}"
    );
    assert!(!output.contains("$ du -sh ~"), "{output}");
    assert_eq!(count_occurrences(&output, "需要审批"), 1, "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("Approved req-1"), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
}

fn run_qwen_continuation_deny(label: &str, extra_env: &[(&str, &str)]) -> String {
    let home = temp_shell_home(label);
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
session="sess-cosh-continuation-deny"
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-continuation-deny","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-continuation-deny","message":{"content":[{"type":"text","text":"Continuation summarized existing shell evidence in plan mode."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-continuation-deny","is_error":false,"result":"done"}'
    exit 0
    ;;
esac
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-continuation-deny","model":"qwen-test"}'
while IFS= read -r line; do
  case "$line" in
    *ShellCommandCompleted*)
      printf '%s\n' '{"type":"control_request","request_id":"ctrl-next","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"du -sh ~"},"tool_use_id":"toolu-next"}}'
      if IFS= read -r response; then
        case "$response" in
          *'"behavior":"deny"'*)
            printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-continuation-deny","message":{"content":[{"type":"text","text":"Continuation summarized existing shell evidence after tool denial."}]}}'
            printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-continuation-deny","is_error":false,"result":"done"}'
            exit 0
            ;;
        esac
      fi
      exit 2
      ;;
    *provider-auto-second-tool*)
      printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"ssh -V"},"tool_use_id":"toolu-1"}}'
      sleep 30
      exit 0
      ;;
  esac
done
exit 0
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let mut env = vec![("HOME", home_str.as_str()), ("PATH", path.as_str())];
    env.extend_from_slice(extra_env);
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &env,
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-auto-second-tool\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\n".to_vec(), Duration::from_millis(1_500)),
            (b"exit 0\n".to_vec(), Duration::from_millis(4_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    output
}

#[test]
fn raw_cli_qwen_shell_without_advertised_host_capability_uses_foreground_shell() {
    let home = temp_shell_home("qwen-silent-resume-fallback");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-silent-resume","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-silent-resume","message":{"content":[{"type":"text","text":"Fresh continuation summarized shell evidence after silent resume."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-silent-resume","is_error":false,"result":"done"}'
    exit 0
    ;;
esac

case " $* " in
  *" --resume "*)
    sleep 30
    exit 0
    ;;
esac

printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-silent-resume","model":"qwen-test"}'
while IFS= read -r line; do
  case "$line" in
    *ShellCommandCompleted*)
      printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-silent-resume","message":{"content":[{"type":"text","text":"Fresh continuation summarized shell evidence after silent resume."}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-silent-resume","is_error":false,"result":"done"}'
      exit 0
      ;;
	    *provider-real-resume-silent*)
	      printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
	      if IFS= read -r response; then
	        case "$response" in
		          *'"request_id":"ctrl-1"'*'"behavior":"host_executed_shell"'*)
		            printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-silent-resume","message":{"content":[{"type":"text","text":"Qwen consumed foreground shell evidence."}]}}'
		            printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-silent-resume","is_error":false,"result":"done"}'
		            exit 0
		            ;;
	        esac
	      fi
		      printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-silent-resume","is_error":true,"result":"missing host_executed_shell result"}'
		      exit 1
		      ;;
  esac
done
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
            ("COSH_AGENT_START_TIMEOUT_SECS", "2"),
        ],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-real-resume-silent\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(3_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("Qwen consumed foreground shell evidence."),
        "{output}"
    );
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(
        !output.contains("Using a fresh provider turn for shell evidence recovery."),
        "{output}"
    );
    assert!(
        !output.contains("Agent timed out: No provider response"),
        "{output}"
    );
}

#[test]
fn raw_cli_qwen_control_shell_result_uses_foreground_transcript() {
    let home = temp_shell_home("qwen-foreground-tool-result");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-qwen-foreground-tool-result","model":"qwen-test"}'
while IFS= read -r line; do
  case "$line" in
    *qwen-foreground-tool-result*)
      printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
      if IFS= read -r response; then
        case "$response" in
          *'"request_id":"ctrl-1"'*'"behavior":"host_executed_shell"'*)
	            printf '%s\n' '{"type":"assistant","session_id":"sess-qwen-foreground-tool-result","message":{"content":[{"type":"text","text":"Qwen saw foreground shell output."}]}}'
	            printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-qwen-foreground-tool-result","is_error":false,"result":"done"}'
            exit 0
            ;;
        esac
      fi
	      printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-qwen-foreground-tool-result","is_error":true,"result":"missing host_executed_shell result"}'
	      exit 1
      ;;
  esac
done
exit 0
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"qwen-foreground-tool-result\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(3_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(output.contains("$ df -h"), "{output}");
    assert!(output.contains("Filesystem"), "{output}");
    assert!(
        output.contains("Qwen saw foreground shell output."),
        "{output}"
    );
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(
        !output.contains("Tool output: stdout captured; [Details]"),
        "{output}"
    );
    assert!(
        !output.contains("missing host_executed_shell result"),
        "{output}"
    );
}

#[test]
fn raw_cli_cwd_scoped_qwen_shell_uses_foreground_without_half_open_resume() {
    let home = temp_shell_home("qwen-cwd-resume-fallback");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case "$*" in
  *ShellCommandCompleted*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-cwd-resume","model":"qwen-test"}'
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-cwd-resume","message":{"content":[{"type":"text","text":"Cwd-scoped fresh continuation summarized shell evidence."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-cwd-resume","is_error":false,"result":"done"}'
    exit 0
    ;;
esac

case " $* " in
  *" --resume "*)
    sleep 30
    exit 0
    ;;
esac

printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-cwd-resume","model":"qwen-test"}'
while IFS= read -r line; do
  case "$line" in
	    *provider-cwd-resume-silent*)
	      printf '%s\n' '{"type":"control_request","request_id":"ctrl-1","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-1"}}'
	      if IFS= read -r response; then
	        case "$response" in
		          *'"request_id":"ctrl-1"'*'"behavior":"host_executed_shell"'*)
		            printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-cwd-resume","message":{"content":[{"type":"text","text":"Cwd-scoped foreground shell evidence handled safe shell."}]}}'
		            printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-cwd-resume","is_error":false,"result":"done"}'
		            exit 0
	            ;;
	        esac
	      fi
		      printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-cwd-resume","is_error":true,"result":"missing host_executed_shell result"}'
		      exit 1
	      ;;
  esac
done
exit 0
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_current_dir_and_delayed_input(
        "qwen",
        &[],
        &[
            ("HOME", &home_str),
            ("PATH", &path),
            ("COSH_AGENT_START_TIMEOUT_SECS", "2"),
        ],
        &home,
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"provider-cwd-resume-silent\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"exit 0\n".to_vec(), Duration::from_millis(3_000)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Auto-approved req-1"), "{output}");
    assert!(output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        output.contains("Cwd-scoped foreground shell evidence handled safe shell."),
        "{output}"
    );
    assert!(
        !output.contains("Provider-native shell tool allowed"),
        "{output}"
    );
    assert!(
        !output.contains("Using a fresh provider turn for shell evidence recovery."),
        "{output}"
    );
    assert!(
        !output.contains("Agent timed out: No provider response"),
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

#[test]
fn raw_cli_provider_tool_interactive_escape_hatch_requires_explicit_send() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"?? provider interactive failure\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"/details out-1\n/details tool-2\n".to_vec(),
                Duration::from_millis(2_500),
            ),
            (
                b"/send-to-shell handoff-1\n".to_vec(),
                Duration::from_millis(1_000),
            ),
            (
                b"echo after-handoff\n".to_vec(),
                Duration::from_millis(2_000),
            ),
            (
                b"/details handoff-1\nexit\n".to_vec(),
                Duration::from_millis(1_000),
            ),
        ],
    );

    assert!(output.contains("may require foreground shell"), "{output}");
    assert!(output.contains("[Send to shell]"), "{output}");
    assert!(output.contains("handoff-1"), "{output}");
    assert!(
        output.contains("Tool error: sudo: a terminal is required"),
        "{output}"
    );
    assert!(output.contains("sudo: a terminal is required"), "{output}");
    assert!(output.contains("Sending to shell"), "{output}");
    assert!(output.contains("$ git status"), "{output}");
    assert!(output.contains("Activity details handoff-1"), "{output}");
    assert!(
        output.contains("evidence: ShellCommandCompleted"),
        "{output}"
    );
    assert!(
        output.contains("execution_path: foreground_shell_pty"),
        "{output}"
    );
    assert!(output.contains("preview_hash: fnv1a64:"), "{output}");
    assert!(output.contains("actor: user"), "{output}");
    assert!(output.contains("source: send_to_shell"), "{output}");
    assert!(output.contains("tool_use_id: toolu-tty"), "{output}");
    assert!(output.contains("redaction_status: ref_only"), "{output}");
    assert!(
        output.contains("start a new Agent turn after the shell command completes"),
        "{output}"
    );
    assert!(output.contains("after-handoff"), "{output}");
    assert!(!output.contains("Tool result for request"), "{output}");
    assert!(!output.contains("Governance:"), "{output}");
}

#[test]
fn raw_cli_provider_tool_send_to_shell_uses_zh_language_env() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (
                b"?? provider interactive failure\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"/details out-1\n/details tool-2\n".to_vec(),
                Duration::from_millis(2_500),
            ),
            (
                b"/send-to-shell handoff-1\n".to_vec(),
                Duration::from_millis(1_000),
            ),
            (
                b"echo after-zh-handoff\n".to_vec(),
                Duration::from_millis(2_000),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(1_000)),
        ],
    );

    assert!(output.contains("可能需要前台 shell"), "{output}");
    assert!(output.contains("正在发送到 shell"), "{output}");
    assert!(
        output.contains("handoff-1 将在前台 shell 中运行。"),
        "{output}"
    );
    assert!(output.contains("$ git status"), "{output}");
    assert!(output.contains("after-zh-handoff"), "{output}");
    assert!(!output.contains("Sending to shell"), "{output}");
    assert!(
        !output.contains("will run in the foreground shell"),
        "{output}"
    );
    assert!(!output.contains("may require foreground shell"), "{output}");
    assert!(!output.contains("bash: /send-to-shell"), "{output}");
}

#[test]
fn raw_cli_select_before_recommendation_is_display_only_noop() {
    let output = run_raw_cli_with_input("fake", "/select 1\necho after-early-select\nexit\n");

    assert!(output.contains("No selectable recommendation is available yet"));
    assert!(output.contains("after-early-select"));
    assert!(!output.contains("The command ls "));
}

#[test]
fn raw_cli_zh_select_before_recommendation_uses_catalog_fallback() {
    let output = run_raw_cli_with_env(
        "fake",
        "/select 1\necho after-early-select\nexit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("没有可选择的推荐"), "{output}");
    assert!(output.contains("当前还没有可选择的推荐"), "{output}");
    assert!(output.contains("after-early-select"), "{output}");
    assert!(!output.contains("No selectable recommendation"), "{output}");
    assert!(
        !output.contains("No selectable recommendation is available yet"),
        "{output}"
    );
    assert!(!output.contains("The command ls "), "{output}");
    assert_no_migrated_english_ui_labels(&output, RENDERER_ZH_FORBIDDEN_UI);
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
        output.contains(
            "missing-id is not available; use a Details action with an approval or activity id"
        ),
        "{output}"
    );
    assert!(output.contains("after-missing-details"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
}

#[test]
fn raw_cli_missing_details_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/details missing-id\n\
         echo after-missing-details\n\
         exit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("详情不可用"), "{output}");
    assert!(
        output.contains("missing-id 不可用；请对审批或活动 id 使用 Details 操作"),
        "{output}"
    );
    assert!(!output.contains("Details unavailable"), "{output}");
    assert!(output.contains("after-missing-details"), "{output}");
    assert!(!output.contains("bash: /details"), "{output}");
    assert_no_migrated_english_ui_labels(&output, DETAILS_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_missing_send_to_shell_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/send-to-shell missing-handoff\n\
         echo after-missing-handoff\n\
         exit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("Shell handoff 未找到"), "{output}");
    assert!(
        output.contains("missing-handoff 不可用；请先对 provider tool failure 使用 Details 操作"),
        "{output}"
    );
    assert!(!output.contains("Shell handoff not found"), "{output}");
    assert!(output.contains("after-missing-handoff"), "{output}");
    assert!(!output.contains("bash: /send-to-shell"), "{output}");
}

#[test]
fn raw_cli_help_renders_slash_command_reference() {
    let output = run_raw_cli_with_input("fake", "/help\necho after-help\nexit\n");

    assert!(output.contains("Slash commands"), "{output}");
    assert!(output.contains("Config"), "{output}");
    assert!(output.contains("Modes"), "{output}");
    assert!(output.contains("Hooks"), "{output}");
    assert!(!output.contains("Agent"), "{output}");
    assert!(!output.contains("Inspect"), "{output}");
    assert!(!output.contains("Recommendations"), "{output}");
    assert!(
        output.contains("/config language [auto|en-US|zh-CN]"),
        "{output}"
    );
    assert!(
        output.contains("/mode approval [recommend|auto|trust]"),
        "{output}"
    );
    assert!(
        output.contains("/mode analysis [smart|auto|manual]"),
        "{output}"
    );
    assert!(!output.contains("/agent"), "{output}");
    assert!(!output.contains("/explain"), "{output}");
    assert!(!output.contains("/cancel"), "{output}");
    assert!(!output.contains("/details <id>"), "{output}");
    assert!(!output.contains("/audit"), "{output}");
    assert!(!output.contains("/select N"), "{output}");
    assert!(!output.contains("/copy N"), "{output}");
    assert!(!output.contains("/mode [recommend|auto|trust]"), "{output}");
    assert!(!output.contains("/skill"), "{output}");
    assert!(
        !output.contains("/approval-mode [suggest|ask|auto|trust]"),
        "{output}"
    );
    assert!(!output.contains("advanced legacy governance"), "{output}");
    assert!(!output.contains("/allow <n>"), "{output}");
    assert!(!output.contains("[ask|auto]alias"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash commands"), "{output}");
    assert!(output.contains("Mode: auto."), "{output}");
    assert!(output.contains("after-help"), "{output}");
    assert!(!output.contains("bash: /help"), "{output}");
}

#[test]
fn raw_cli_unknown_slash_suggests_nearest_canonical_command() {
    let output = run_raw_cli_with_input("fake", "/hep\necho after-unknown\nexit\n");

    assert!(output.contains("Unknown slash command: /hep"), "{output}");
    assert!(output.contains("Did you mean /help?"), "{output}");
    assert!(!output.contains("/approval-mode"), "{output}");
    assert!(output.contains("after-unknown"), "{output}");
    assert!(!output.contains("bash: /hep"), "{output}");
}

#[test]
fn raw_cli_unknown_slash_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/hep\n\
         echo after-unknown-zh\n\
         exit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("未知 slash 命令: /hep"), "{output}");
    assert!(output.contains("你是不是想用 /help？"), "{output}");
    assert!(output.contains("使用 /help 查看可用命令。"), "{output}");
    assert!(!output.contains("Unknown slash command"), "{output}");
    assert!(!output.contains("Did you mean /help?"), "{output}");
    assert!(
        !output.contains("Use /help to see available commands."),
        "{output}"
    );
    assert!(output.contains("after-unknown-zh"), "{output}");
    assert!(!output.contains("bash: /hep"), "{output}");
    assert_no_migrated_english_ui_labels(&output, SLASH_CONFIG_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_help_and_mode_use_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/help\n/mode\n/mode language zh-CN\nexit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("Slash 命令"), "{output}");
    assert!(output.contains("配置"), "{output}");
    assert!(output.contains("配置界面语言"), "{output}");
    assert!(output.contains("审批: auto"), "{output}");
    assert!(output.contains("分析: smart"), "{output}");
    assert!(
        output.contains("语言是持久化配置，不是运行时模式。"),
        "{output}"
    );
    assert!(
        output.contains("使用 /config language [auto|en-US|zh-CN]。"),
        "{output}"
    );
    assert!(!output.contains("bash: /help"), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
    assert_no_migrated_english_ui_labels(&output, SLASH_CONFIG_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_mode_approval_and_analysis_use_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/mode approval trust\n\
         /mode approval trust confirm\n\
         /mode analysis auto\n\
         /mode analysis manual\n\
         exit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("需要确认 trust 模式"), "{output}");
    assert!(
        output.contains("运行 /mode approval trust confirm 显式启用。"),
        "{output}"
    );
    assert!(output.contains("模式已设置为 trust。"), "{output}");
    assert!(output.contains("分析模式"), "{output}");
    assert!(
        output.contains("命令失败时评估 hooks；自动触发 Agent 分析。"),
        "{output}"
    );
    assert!(
        output.contains("已禁用 hooks 和自动分析；使用 slash 命令手动触发。"),
        "{output}"
    );
    assert!(!output.contains("bash: /mode"), "{output}");
    assert_no_migrated_english_ui_labels(&output, MODE_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_mode_approval_card_uses_zh_language_env() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"/mode approval\n".to_vec(), Duration::from_millis(500)),
            (b"\x1b[D\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("用户模式"), "{output}");
    assert!(output.contains("当前: auto"), "{output}");
    assert!(output.contains("只解释和建议"), "{output}");
    assert!(output.contains("按键: Left/Right 选择"), "{output}");
    assert!(output.contains("模式已设置为 recommend。"), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
    assert_no_migrated_english_ui_labels(&output, MODE_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_config_language_save_applies_to_current_session_help() {
    let home = temp_shell_home("config-language-current-session");
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str)],
        vec![
            (b"/config language zh-CN\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"/help\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("配置已保存"), "{output}");
    assert!(output.contains("当前会话语言: zh-CN。"), "{output}");
    assert!(output.contains("Slash 命令"), "{output}");
    assert!(output.contains("配置"), "{output}");
    assert!(!output.contains("Config saved"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
    assert!(!output.contains("bash: /help"), "{output}");
    assert_no_migrated_english_ui_labels(&output, SLASH_CONFIG_ZH_FORBIDDEN_UI);
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
    assert!(output.contains("language:"), "{output}");
    assert!(output.contains("debug activity: off"), "{output}");
    assert!(output.contains("Use /config language"), "{output}");
    assert!(output.contains("Audit"), "{output}");
    assert!(
        output.contains("Approval decisions are available with Details actions"),
        "{output}"
    );
    assert!(output.contains("after-info-slash"), "{output}");
    assert!(!output.contains("bash: /skill"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
    assert!(!output.contains("bash: /audit"), "{output}");
}

#[test]
fn raw_cli_config_summary_reads_language_from_user_config() {
    let home = temp_shell_home("config-language-summary");
    write_cosh_config(
        &home,
        r#"
[ui]
language = "zh-CN"
debug = true
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_env(
        "fake",
        "/config\nexit\n",
        &[("HOME", &home_str), ("COSH_SHELL_LANG", RAW_CLI_UNSET_ENV)],
    );

    assert!(output.contains("配置"), "{output}");
    assert!(output.contains("语言: zh-CN 来源: config"), "{output}");
    assert!(output.contains("调试活动: on"), "{output}");
    assert!(output.contains("config.toml"), "{output}");
    assert!(
        output.contains("使用 /config language [auto|en-US|zh-CN]"),
        "{output}"
    );
    assert!(!output.contains("bash: /config"), "{output}");
}

#[test]
fn raw_cli_config_language_errors_use_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/config language nope\n/config unknown\nexit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("配置"), "{output}");
    assert!(output.contains("无效语言: nope"), "{output}");
    assert!(output.contains("支持: auto, en-US, zh-CN。"), "{output}");
    assert!(output.contains("未知配置项: unknown"), "{output}");
    assert!(!output.contains("Invalid language"), "{output}");
    assert!(!output.contains("Unknown config key"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
    assert_no_migrated_english_ui_labels(&output, SLASH_CONFIG_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_config_language_direct_set_saves_after_confirmation() {
    let home = temp_shell_home("config-language-save");
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str), ("COSH_SHELL_LANG", RAW_CLI_UNSET_ENV)],
        vec![
            (b"/config language zh-CN\n".to_vec(), Duration::ZERO),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"/config\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    let config_path = home.join(".config/cosh/config.toml");
    let content = fs::read_to_string(&config_path).expect("read saved config");
    assert!(content.contains("[ui]"), "{content}");
    assert!(content.contains("language = \"zh-CN\""), "{content}");
    assert!(output.contains("Save config?"), "{output}");
    assert!(output.contains("配置已保存"), "{output}");
    assert!(
        output.contains("保存的设置会在下次启动时生效。"),
        "{output}"
    );
    assert!(output.contains("语言: zh-CN 来源: config"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
}

#[test]
fn raw_cli_config_language_selector_saves_after_confirmation() {
    let home = temp_shell_home("config-language-selector-save");
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str), ("COSH_SHELL_LANG", RAW_CLI_UNSET_ENV)],
        vec![
            (b"/config language\n".to_vec(), Duration::ZERO),
            (b"\x1b[C\x1b[C\n".to_vec(), Duration::from_millis(1_200)),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"/config\n".to_vec(), Duration::from_millis(200)),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    let config_path = home.join(".config/cosh/config.toml");
    let content = fs::read_to_string(&config_path).expect("read saved config");
    assert!(content.contains("language = \"zh-CN\""), "{content}");
    assert!(output.contains("Language"), "{output}");
    assert!(output.contains("zh-CN"), "{output}");
    assert!(output.contains("Save config?"), "{output}");
    assert!(output.contains("配置已保存"), "{output}");
    assert!(output.contains("语言: zh-CN 来源: config"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
    assert!(!output.contains("^[[C"), "{output}");
}

#[test]
fn raw_cli_config_language_selector_cancel_does_not_write_file() {
    let home = temp_shell_home("config-language-selector-cancel");
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str)],
        vec![
            (b"/config language\n".to_vec(), Duration::ZERO),
            (b"\x1b\n".to_vec(), Duration::from_millis(1_200)),
            (
                b"echo after-config-cancel\n".to_vec(),
                Duration::from_millis(200),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(!home.join(".config/cosh/config.toml").exists());
    assert!(output.contains("Language"), "{output}");
    assert!(output.contains("Config unchanged"), "{output}");
    assert!(output.contains("No config file was changed."), "{output}");
    assert!(output.contains("after-config-cancel"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
}

#[test]
fn raw_cli_config_env_language_override_does_not_rewrite_file() {
    let home = temp_shell_home("config-language-env");
    write_cosh_config(
        &home,
        r#"
[ui]
language = "zh-CN"
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_env(
        "fake",
        "/config\nexit\n",
        &[("HOME", &home_str), ("COSH_SHELL_LANG", "en-US")],
    );

    let config_path = home.join(".config/cosh/config.toml");
    let content = fs::read_to_string(&config_path).expect("read saved config");
    assert!(content.contains("language = \"zh-CN\""), "{content}");
    assert!(output.contains("language: en-US source: env"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
}

#[test]
fn raw_cli_bare_slash_is_noop_without_hint_card() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"/\n".to_vec(), Duration::ZERO),
            (
                b"echo after-bare-slash\n".to_vec(),
                Duration::from_millis(200),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(!output.contains("Slash command hint"), "{output}");
    assert!(!output.contains("/help  /mode"), "{output}");
    assert!(!output.contains("bash: /"), "{output}");
    assert!(output.contains("after-bare-slash"), "{output}");
}

#[test]
fn raw_cli_slash_prefix_renders_hint_without_leaking_to_shell() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mo\n\
         echo after-slash-hint\n\
         exit\n",
    );

    assert!(output.contains("Slash command hint"), "{output}");
    assert!(
        output.contains("/mode approval [recommend|auto|trust] - change approval mode"),
        "{output}"
    );
    assert!(!output.contains("/allow <n>"), "{output}");
    assert!(output.contains("Prefix: /mo"), "{output}");
    assert!(output.contains("after-slash-hint"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash command"), "{output}");
    assert!(!output.contains("bash: /:"), "{output}");
    assert!(!output.contains("bash: /mo"), "{output}");
}

#[test]
fn raw_cli_slash_cards_wrap_long_text_and_restore_prompt() {
    let output = run_raw_cli_with_env(
        "fake",
        "/help\n\
         echo after-long-slash\n\
         exit\n",
        &[("TERM", "xterm-256color"), ("COSH_SHELL_WIDTH", "72")],
    );

    assert!(output.contains("Slash commands"), "{output}");
    assert!(
        output.contains("/mode approval [recommend|auto|trust]"),
        "{output}"
    );
    assert!(output.contains("change approval mode"), "{output}");
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
        "/mode approval trust\n\
         /mode approval trust confirm\n\
         /help\n\
         /approval-mode recommend\n\
         /mode auto\n\
         /mode invalid\n\
         echo after-mode\n\
         exit\n",
    );

    assert!(output.contains("Approval mode"), "{output}");
    assert!(output.contains("Trust confirmation required"), "{output}");
    assert!(
        output.contains("Run /mode approval trust confirm"),
        "{output}"
    );
    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Mode: trust. Strategy: smart."), "{output}");
    assert!(
        output.contains("/approval-mode is not supported."),
        "{output}"
    );
    assert!(output.contains("Use /mode approval recommend."), "{output}");
    assert!(output.contains("/mode auto is not supported."), "{output}");
    assert!(output.contains("Use /mode approval auto."), "{output}");
    assert!(!output.contains("Mode set to recommend."), "{output}");
    assert!(!output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Unknown mode: invalid"), "{output}");
    assert!(
        output.contains("Use /mode approval recommend|auto|trust"),
        "{output}"
    );
    assert!(output.contains("after-mode"), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
    assert!(!output.contains("bash: /approval-mode"), "{output}");
}

#[test]
fn raw_cli_mode_root_and_language_guidance_are_canonical() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mode\n\
         /mode language zh-CN\n\
         echo after-mode-guidance\n\
         exit\n",
    );

    assert!(output.contains("Modes"), "{output}");
    assert!(output.contains("approval: auto"), "{output}");
    assert!(output.contains("analysis: smart"), "{output}");
    assert!(
        output.contains("Use /mode approval [recommend|auto|trust]"),
        "{output}"
    );
    assert!(
        output.contains("Language is persistent config, not a runtime mode."),
        "{output}"
    );
    assert!(
        output.contains("Use /config language [auto|en-US|zh-CN]."),
        "{output}"
    );
    assert!(output.contains("after-mode-guidance"), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
}

#[test]
fn raw_cli_mode_slash_panel_selects_recommend_with_card_input() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"/mode approval\n".to_vec(), Duration::from_millis(500)),
            (b"\x1b[D\n".to_vec(), Duration::from_millis(1_000)),
            (b"exit\n".to_vec(), Duration::from_millis(200)),
        ],
    );

    assert!(output.contains("User mode"), "{output}");
    assert!(output.contains("Current: auto"), "{output}");
    assert!(output.contains("> [ auto"), "{output}");
    assert!(output.contains("Mode set to recommend."), "{output}");
    assert!(!output.contains("bash: /mode"), "{output}");
    assert!(!output.contains("bash: \u{1b}"), "{output}");
}

#[test]
fn raw_cli_suggest_mode_keeps_tool_requests_display_only() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mode approval recommend\n\
         ?? request tool approval\n\
         exit\n",
    );

    assert!(output.contains("Mode set to recommend."), "{output}");
    assert!(output.contains("Received shell prompt request"), "{output}");
    assert!(!output.contains("Approval req-"), "{output}");
    assert!(!output.contains("Auto-approved"), "{output}");
    assert!(
        !output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
}

#[test]
fn raw_cli_auto_mode_runs_safe_bash_tool_without_approval_panel() {
    let output = run_raw_cli_with_env(
        "fake",
        "/mode approval auto\n\
         ?? request tool approval\n\
         exit\n",
        &[("COSH_SHELL_LANG", "en-US")],
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Deferred req-1"), "{output}");
    assert!(output.contains("$ git status"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("[ Allow once ]"), "{output}");
    assert!(!output.contains("Command result analysis for req-1"));
    assert!(!output.contains("Tool result for request req-1"));
    assert!(
        !output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
}

#[test]
fn raw_cli_trust_mode_runs_bash_tool_without_approval_panel() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"/mode approval trust confirm\n".to_vec(), Duration::ZERO),
            (
                b"?? stream pwd tool approval\n".to_vec(),
                Duration::from_millis(100),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(2_000)),
        ],
    );

    assert!(output.contains("Mode set to trust."), "{output}");
    assert!(output.contains("Deferred req-1"), "{output}");
    assert!(!output.contains("Command result analysis for req-1"));
    assert!(!output.contains("Tool result for request req-1"));
    assert!(!output.contains("Approval req-"), "{output}");
    assert!(!output.contains("[ Allow once ]"), "{output}");
}

#[test]
fn raw_cli_auto_mode_skips_readonly_builtin_tool_approval_panel() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mode approval auto\n\
         ?? request readonly builtin tool\n\
         exit\n",
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(!output.contains("Auto-approved req-"), "{output}");
    assert!(
        output.contains("Read called: Cargo.toml; [Details] tool-1"),
        "{output}"
    );
    assert!(
        output.contains("Grep called: /cosh/ in crates/cosh-shell; [Details] tool-2"),
        "{output}"
    );
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("[ Allow once ]"), "{output}");
    assert!(!output.contains("$ {\"file_path\""), "{output}");
}

#[test]
fn raw_cli_auto_mode_still_asks_for_unsafe_bash_tool() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"/mode approval auto\n".to_vec(), Duration::ZERO),
            (
                b"?? request unsafe tool approval\n".to_vec(),
                Duration::from_millis(500),
            ),
            (b"\x1b".to_vec(), Duration::from_millis(800)),
            (b"exit\n".to_vec(), Duration::from_millis(800)),
        ],
    );

    assert!(output.contains("Mode set to auto."), "{output}");
    assert!(output.contains("Approval required"), "{output}");
    assert!(output.contains("req-1"), "{output}");
    assert!(
        output.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{output}"
    );
    assert!(output.contains("Cancelled req-1"), "{output}");
    assert!(!output.contains("Auto-approved"), "{output}");
    assert!(!output.contains("Approved req-1"), "{output}");
}

#[test]
fn raw_cli_auto_mode_skips_exact_trusted_command() {
    let home = temp_shell_home("trusted-exact");
    write_cosh_config(
        &home,
        r#"approval.trusted_command = "touch /tmp/cosh-shell-fake-action-should-not-run""#,
    );
    let home_str = home.to_string_lossy().to_string();
    let _ = fs::remove_file("/tmp/cosh-shell-fake-action-should-not-run");

    let output = run_raw_cli_with_env(
        "fake",
        "?? request unsafe tool approval\nexit\n",
        &[("HOME", &home_str)],
    );

    assert!(output.contains("Deferred req-1"), "{output}");
    assert!(output.contains("$ touch /tmp/cosh-shell-fake-action-should-not-run"));
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("Trusted req-1"), "{output}");

    let _ = fs::remove_file("/tmp/cosh-shell-fake-action-should-not-run");
}

#[test]
fn raw_cli_auto_mode_trusted_command_requires_exact_match() {
    let home = temp_shell_home("trusted-exact-miss");
    write_cosh_config(
        &home,
        r#"approval.trusted_command = "touch /tmp/cosh-shell-fake-action-should-not-run --dry-run""#,
    );
    let home_str = home.to_string_lossy().to_string();
    let _ = fs::remove_file("/tmp/cosh-shell-fake-action-should-not-run");

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("HOME", &home_str)],
        vec![
            (
                b"?? request unsafe tool approval\n".to_vec(),
                Duration::ZERO,
            ),
            (b"\x1b".to_vec(), Duration::from_millis(800)),
            (b"exit\n".to_vec(), Duration::from_millis(300)),
        ],
    );

    assert!(output.contains("Approval required"), "{output}");
    assert!(!output.contains("Trusted req-1"), "{output}");
    assert!(!output.contains("Auto-approved req-1"), "{output}");

    let _ = fs::remove_file("/tmp/cosh-shell-fake-action-should-not-run");
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
fn raw_cli_cancel_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/cancel\n\
         echo after-cancel\n\
         exit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("Agent 已取消"), "{output}");
    assert!(output.contains("当前没有等待取消的 Agent 运行"), "{output}");
    assert!(output.contains("Shell 保持可用。"), "{output}");
    assert!(output.contains("after-cancel"), "{output}");
    assert!(!output.contains("Agent cancelled"), "{output}");
    assert!(
        !output.contains("no active Agent run is currently waiting for cancellation"),
        "{output}"
    );
    assert!(!output.contains("Shell remains active."), "{output}");
    assert!(!output.contains("bash: /cancel"), "{output}");
}

#[test]
fn raw_cli_cancel_stops_active_agent_run_and_keeps_shell_usable() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? hold test slow agent\n".to_vec(), Duration::ZERO),
            (b"/cancel\n".to_vec(), Duration::from_millis(1_000)),
            (
                b"echo after-active-cancel\nexit\n".to_vec(),
                Duration::from_millis(700),
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
fn raw_cli_active_cancel_uses_zh_language_env() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"?? text then wait slow agent\n".to_vec(), Duration::ZERO),
            (b"/cancel\n".to_vec(), Duration::from_millis(700)),
            (
                b"echo after-active-cancel\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert!(output.contains("Agent 取消请求已发送"), "{output}");
    assert!(output.contains("正在停止 active Agent 运行..."), "{output}");
    assert!(output.contains("Agent 已取消"), "{output}");
    assert!(output.contains("原因: 用户请求取消"), "{output}");
    assert!(output.contains("after-active-cancel"), "{output}");
    assert!(!output.contains("Agent cancellation requested"), "{output}");
    assert!(!output.contains("Stopping active Agent run"), "{output}");
    assert!(
        !output.contains("Reason: user requested cancellation"),
        "{output}"
    );
    assert!(!output.contains("bash: /cancel"), "{output}");
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
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("No provider response within"), "{output}");
    assert!(!output.contains("cosh-osc$ cosh-osc$"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_interrupts_foreground_command_without_agent_cancel() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"sleep 5\n".to_vec(), Duration::from_millis(500)),
            (vec![0x03], Duration::from_millis(500)),
            (
                b"echo after-foreground-ctrl-c\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert!(output.contains("sleep 5"), "{output}");
    assert!(output.contains("after-foreground-ctrl-c"), "{output}");
    assert!(!output.contains("Agent cancellation requested"), "{output}");
    assert!(
        !output.contains("Reason: user requested cancellation"),
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
    assert!(!output.contains("The command sleep 5 failed"), "{output}");
}

#[test]
fn raw_cli_second_ctrl_c_recovers_ignored_foreground_command() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"bash -c 'trap \"\" INT; trap \"exit 0\" QUIT; while :; do sleep 1; done'\n"
                    .to_vec(),
                Duration::ZERO,
            ),
            (vec![0x03], Duration::from_millis(500)),
            (vec![0x03], Duration::from_millis(500)),
            (
                b"echo after-foreground-escalation\nexit\n".to_vec(),
                Duration::from_millis(1_000),
            ),
        ],
    );

    assert!(output.contains("after-foreground-escalation"), "{output}");
    assert!(!output.contains("Agent cancellation requested"), "{output}");
    assert!(
        !output.contains("Reason: user requested cancellation"),
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_active_agent_cancel_is_idempotent() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? very slow agent\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(500)),
            (vec![0x03], Duration::from_millis(100)),
            (
                b"echo after-double-agent-ctrl-c\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert_eq!(
        count_occurrences(&output, "Agent cancellation requested"),
        1,
        "{output}"
    );
    assert_eq!(
        count_occurrences(&output, "Stopping active Agent run"),
        1,
        "{output}"
    );
    assert_eq!(
        count_occurrences(&output, "Reason: user requested cancellation"),
        1,
        "{output}"
    );
    assert!(output.contains("after-double-agent-ctrl-c"), "{output}");
    assert!(!output.contains("Slow fake response for"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("cosh-osc$ cosh-osc$"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_drops_unclosed_request_block_before_prompt() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (
                b"?? slow unclosed request then wait\n".to_vec(),
                Duration::ZERO,
            ),
            (vec![0x03], Duration::from_millis(500)),
            (
                b"echo after-unclosed-request-cancel\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("after-unclosed-request-cancel"), "{output}");
    assert!(!output.contains("```cosh-request"), "{output}");
    assert!(!output.contains("Agent Requested Evidence"), "{output}");
    assert!(
        !output.contains("Evidence history index received"),
        "{output}"
    );
    assert!(!output.contains("cosh-osc$ cosh-osc$"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_drops_late_provider_cancel_timeout_artifact() {
    let home = temp_shell_home("qwen-late-cancel-artifact");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
trap 'printf "%s\n" "{\"type\":\"result\",\"subtype\":\"error\",\"session_id\":\"sess-late-cancel\",\"is_error\":true,\"result\":\"Agent timed out: No provider response within 20s\"}"; exit 0' TERM INT HUP
read -r init || exit 0
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-late-cancel","model":"qwen-test"}'
read -r user_message || exit 0
case "$user_message" in
  *provider-cancel-artifact*)
    while :; do sleep 1; done
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-late-cancel","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"?? provider-cancel-artifact\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(700)),
            (
                b"echo after-provider-cancel\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("after-provider-cancel"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(!output.contains("No provider response within"), "{output}");
    assert!(
        !output.contains("Using a fresh provider turn for shell evidence recovery."),
        "{output}"
    );
}

#[test]
fn raw_cli_ctrl_c_archives_provider_cancel_artifact_in_details() {
    let home = temp_shell_home("qwen-cancel-artifact-details");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/usr/bin/env perl
use strict;
use warnings;
$| = 1;
$SIG{TERM} = sub {
  print "PROVIDER_CANCEL_STDOUT_ARTIFACT\n";
  print STDERR "PROVIDER_CANCEL_STDERR_ARTIFACT\n";
  exit 0;
};
$SIG{INT} = $SIG{TERM};
$SIG{HUP} = $SIG{TERM};
my $init = <STDIN>;
exit 0 unless defined $init;
print "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"init-1\",\"response\":{\"subtype\":\"initialize\",\"capabilities\":{\"can_handle_can_use_tool\":true,\"can_handle_host_executed_shell_tool_result\":true}}}}\n";
print "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-cancel-artifact-details\",\"model\":\"qwen-test\"}\n";
my $user_message = <STDIN>;
exit 0 unless defined $user_message;
if ($user_message =~ /provider-cancel-artifact-details/) {
  while (1) { sleep 1; }
}
print "{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"sess-cancel-artifact-details\",\"is_error\":false,\"result\":\"ignored\"}\n";
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (
                b"?? provider-cancel-artifact-details\n".to_vec(),
                Duration::ZERO,
            ),
            (vec![0x03], Duration::from_millis(1_500)),
            (
                b"/details provider-cancel-1\nexit\n".to_vec(),
                Duration::from_millis(4_000),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Details: provider-cancel-1"), "{output}");
    assert!(output.contains("Provider cancel details"), "{output}");
    assert!(
        output.contains("PROVIDER_CANCEL_STDOUT_ARTIFACT"),
        "{output}"
    );
    assert!(
        output.contains("PROVIDER_CANCEL_STDERR_ARTIFACT"),
        "{output}"
    );
    let details_pos = output
        .find("Provider cancel details")
        .expect("details panel");
    assert!(
        !output[..details_pos].contains("PROVIDER_CANCEL_STDOUT_ARTIFACT"),
        "{output}"
    );
    assert!(
        !output[..details_pos].contains("PROVIDER_CANCEL_STDERR_ARTIFACT"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_drops_late_provider_tool_request_artifact() {
    let home = temp_shell_home("qwen-late-tool-request-artifact");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
trap 'printf "%s\n" "{\"type\":\"control_request\",\"request_id\":\"late-ctrl\",\"request\":{\"subtype\":\"can_use_tool\",\"tool_name\":\"run_shell_command\",\"input\":{\"command\":\"echo SHOULD_NOT_RUN\"},\"tool_use_id\":\"late-tool\"}}"; exit 0' TERM INT HUP
read -r init || exit 0
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-late-tool","model":"qwen-test"}'
read -r user_message || exit 0
case "$user_message" in
  *provider-cancel-late-tool*)
    while :; do sleep 1; done
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-late-tool","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"?? provider-cancel-late-tool\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(700)),
            (
                b"echo after-late-tool-cancel\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("after-late-tool-cancel"), "{output}");
    assert!(!output.contains("SHOULD_NOT_RUN"), "{output}");
    assert!(!output.contains("late-ctrl"), "{output}");
    assert!(!output.contains("echo SHOULD_NOT_RUN"), "{output}");
    assert!(!output.contains("Approval required"), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_drops_late_provider_tool_error_artifact() {
    let home = temp_shell_home("qwen-late-tool-error-artifact");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
trap 'printf "%s\n" "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_cancel\",\"is_error\":true,\"content\":\"Tool error: Dispatcher shutdown\"}]}}"; exit 0' TERM INT HUP
read -r init || exit 0
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-late-tool-error","model":"qwen-test"}'
read -r user_message || exit 0
case "$user_message" in
  *provider-cancel-late-tool-error*)
    while :; do sleep 1; done
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-late-tool-error","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (
                b"?? provider-cancel-late-tool-error\n".to_vec(),
                Duration::ZERO,
            ),
            (vec![0x03], Duration::from_millis(700)),
            (
                b"echo after-late-tool-error-cancel\nexit\n".to_vec(),
                Duration::from_millis(500),
            ),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("after-late-tool-error-cancel"), "{output}");
    assert!(!output.contains("Dispatcher shutdown"), "{output}");
    assert!(!output.contains("Tool error:"), "{output}");
    assert!(!output.contains("toolu_cancel"), "{output}");
    assert!(!output.contains("Using a fresh provider turn"), "{output}");
    assert!(!output.contains("failed with exit code"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_does_not_resume_cancelled_provider_session() {
    let home = temp_shell_home("qwen-cancelled-session-not-resumed");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let co_path = bin_dir.join("co");
    write_executable(
        &co_path,
        r#"#!/bin/sh
case " $* " in
  *" --resume cancelled-session "*)
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"cancelled-session","is_error":true,"result":"BAD_RESUME_CANCELLED_SESSION"}'
    exit 1
    ;;
esac
read -r init || exit 0
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"cancelled-session","model":"qwen-test"}'
read -r user_message || exit 0
case "$user_message" in
  *continue*)
    printf '%s\n' "$user_message" > "$HOME/second-prompt.txt"
    printf '%s\n' '{"type":"assistant","session_id":"second-session","message":{"content":[{"type":"text","text":"SECOND RUN WITH CANCEL FACTS"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"second-session","is_error":false,"result":"done"}'
    exit 0
    ;;
  *cancelled-provider-session*)
    while :; do sleep 1; done
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"cancelled-session","is_error":false,"result":"ignored"}'
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", bin_dir.display());
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "qwen",
        &[],
        &[("HOME", &home_str), ("PATH", &path)],
        vec![
            (b"?? cancelled-provider-session\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(700)),
            (
                b"?? continue\nexit\n".to_vec(),
                Duration::from_millis(1_500),
            ),
        ],
    );
    let second_prompt = fs::read_to_string(home.join("second-prompt.txt")).unwrap_or_default();
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("SECOND RUN WITH CANCEL FACTS"), "{output}");
    assert!(
        second_prompt.contains("cancelled: user requested cancellation"),
        "{second_prompt}"
    );
    assert!(!output.contains("BAD_RESUME_CANCELLED_SESSION"), "{output}");
    assert!(!output.contains("Agent timed out:"), "{output}");
    assert!(
        !output.contains("Using a fresh provider turn for shell evidence recovery."),
        "{output}"
    );
}

#[test]
fn raw_cli_ctrl_c_drops_late_fake_question_card() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? late card after cancel\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(300)),
            (
                b"echo after-late-card-cancel\nexit\n".to_vec(),
                Duration::from_millis(1_000),
            ),
        ],
    );

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("after-late-card-cancel"), "{output}");
    assert!(
        !output.contains("LATE QUESTION SHOULD NOT RENDER"),
        "{output}"
    );
    assert!(!output.contains("Agent question"), "{output}");
    assert!(!output.contains("Answer sent"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_drops_late_fake_tool_artifact() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? late artifact after cancel\n".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(300)),
            (
                b"echo after-late-artifact-cancel\nexit\n".to_vec(),
                Duration::from_millis(1_000),
            ),
        ],
    );

    assert!(output.contains("Agent cancellation requested"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("after-late-artifact-cancel"), "{output}");
    assert!(
        !output.contains("LATE TOOL ARTIFACT SHOULD NOT RENDER"),
        "{output}"
    );
    assert!(!output.contains("late-tool"), "{output}");
    assert!(!output.contains("Tool error:"), "{output}");
}

#[test]
fn raw_cli_ctrl_c_clears_queued_failed_command_analysis() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"?? very slow agent\n".to_vec(), Duration::ZERO),
            (
                b"ls /path/that/does/not/exist\n".to_vec(),
                Duration::from_millis(200),
            ),
            (vec![0x03], Duration::from_millis(500)),
            (
                b"echo after-queued-cancel\nexit\n".to_vec(),
                Duration::from_millis(200),
            ),
        ],
    );

    assert!(output.contains("Agent queued"), "{output}");
    assert!(output.contains("Agent cancelled"), "{output}");
    assert!(output.contains("after-queued-cancel"), "{output}");
    assert!(
        !output.contains("The command ls /path/that/does/not/exist failed"),
        "{output}"
    );
    assert!(!output.contains("Command failed:"), "{output}");
    assert!(!output.contains("Slow fake response for"), "{output}");
}

#[test]
fn raw_cli_clear_cancels_pending_failed_command_analysis() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n/clear\n/explain last error\necho after-clear\nexit 0\n",
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
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n/shell\n/explain last error\necho after-shell\nexit 0\n",
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
    let output = run_raw_cli_with_delayed_input(
        "fake",
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
fn raw_cli_natural_language_includes_recent_command_facts_without_output_body() {
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
    assert!(
        no_wrap.contains("command=echo shell-context-ok"),
        "{output}"
    );
    assert!(
        no_wrap.contains("output_id=terminal-output://raw-session/cmd-1"),
        "{output}"
    );
    assert!(!no_wrap.contains("command=exit"), "{output}");
    assert!(!no_wrap.contains("preview:"), "{output}");
    assert!(!output.contains("ref="), "{output}");
    assert!(!output.contains("/output-refs/"), "{output}");
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
        compact.contains("Hook routing hints visible to Agent: <none>"),
        "{output}"
    );
    assert!(output.contains("Hook auto-analyzed"), "{output}");
    assert!(
        compact.contains("The command ls /path/that/does/not/exist failed"),
        "{output}"
    );
    assert!(
        compact.contains("command=ls /path/that/does/not/exist"),
        "{output}"
    );
    assert!(
        compact.contains("output_id=terminal-output://raw-session/cmd-1"),
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
    let output = run_raw_cli_with_env("fake", input, &[("COSH_SHELL_LANG", "en-US")]);

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
        &[("COSH_SHELL_LANG", "en-US")],
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

    assert!(output.contains("Hook auto-analyzed"), "{output}");
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

#[test]
fn raw_cli_repeated_failed_command_skips_without_auto_analyzed_notice() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
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
        count_occurrences(&output, "Hook auto-analyzed"),
        1,
        "{output}"
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
        &[("COSH_SHELL_LANG", "zh-CN")],
        vec![
            (b"ls ccc\n".to_vec(), Duration::ZERO),
            (b"ls ccc\n".to_vec(), Duration::from_millis(800)),
            (
                b"echo after-zh-repeat\nexit\n".to_vec(),
                Duration::from_millis(800),
            ),
        ],
    );

    assert_eq!(count_occurrences(&output, "Hook 自动分析"), 1, "{output}");
    assert_eq!(count_occurrences(&output, "已跳过分析"), 1, "{output}");
    assert!(
        output.contains("已跳过 `ls ccc` 的重复失败分析"),
        "{output}"
    );
    assert!(output.contains("Agent 分析正在启动。"), "{output}");
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
        &[("PATH", path.as_str())],
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
        count_occurrences(&output, "Hook auto-analyzed"),
        1,
        "{output}"
    );
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
            (b"exit\n".to_vec(), Duration::from_millis(1800)),
        ],
    );

    assert!(!output.contains("Still working..."));
    assert!(!output.contains("Phase: thinking"));
    assert!(!output.contains("simulating a slow fake Agent run"));
    assert!(output.contains("Slow fake response for: ?? very slow agent"));
}

#[test]
fn raw_cli_agent_marker_invokes_adapter_without_failed_command() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? check current directory\nexit\n",
        &[("COSH_SHELL_LANG", "en-US")],
    );

    assert!(output.contains("Thinking..."));
    assert!(output.contains("Received shell prompt request: ?? check current directory"));
    assert!(!output.contains("command exited with code"));
    assert_no_prompt_between(&output, "Thinking...", "Received shell prompt request");
}

#[test]
fn raw_cli_zh_natural_language_intercept_skips_redundant_notice() {
    let output = run_raw_cli_with_env(
        "fake",
        "帮我看看当前目录\nexit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(!output.contains("AI 请求"), "{output}");
    assert!(!output.contains("正在把输入交给 Agent"), "{output}");
    assert!(
        !output.contains("该输入已在进入 Bash 前被拦截。"),
        "{output}"
    );
    assert!(output.contains("正在思考..."), "{output}");
    assert!(
        output.contains("Received shell prompt request: 帮我看看当前目录"),
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

#[test]
fn raw_cli_no_color_keeps_box_layout_when_terminal_supports_it() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("NO_COLOR", "1"), ("TERM", "xterm-256color")],
        vec![
            (b"ls /path/that/does/not/exist\n".to_vec(), Duration::ZERO),
            (b"exit 0\n".to_vec(), Duration::from_millis(500)),
        ],
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
            ("COSH_SHELL_LANG", "en-US"),
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
fn raw_cli_animation_stops_heartbeat_after_visible_agent_text() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[
            ("COSH_SHELL_LANG", "en-US"),
            ("COSH_SHELL_ANIMATION", "always"),
            ("TERM", "xterm-256color"),
        ],
        vec![
            (b"?? slow text then wait\n".to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(7_500)),
        ],
    );

    assert!(output.contains("⠋ Thinking..."), "{output}");
    assert!(
        output.contains("Slow fake response for: ?? slow text then wait"),
        "{output}"
    );
    assert!(!output.contains("receiving Agent response"), "{output}");
}

#[test]
fn raw_cli_agent_response_renders_markdown_inside_card() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown\nexit\n",
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
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
fn raw_cli_zh_agent_response_renders_markdown_labels() {
    let output = run_raw_cli_with_env(
        "fake",
        "?? render markdown\n?? render markdown table\nexit\n",
        &[("COSH_SHELL_LANG", "zh-CN"), ("TERM", "xterm-256color")],
    );

    assert!(output.contains("╭ Agent 回复"), "{output}");
    assert!(output.contains("│ ┌ 代码: bash"), "{output}");
    assert!(output.contains("│ ┌ 表格"), "{output}");
    assert!(output.contains("│ Project check"), "{output}");
    assert!(output.contains("│ 内存占用 Top 10 分析:"), "{output}");
    assert!(!output.contains("╭ Agent ─"), "{output}");
    assert!(!output.contains("│ ┌ code: bash"), "{output}");
    assert!(!output.contains("│ ┌ table"), "{output}");
    assert_no_migrated_english_ui_labels(&output, RENDERER_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_agent_response_streams_markdown_fragments_inside_card() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
        vec![
            (b"?? stream markdown\n".to_vec(), Duration::ZERO),
            (b"exit\n".to_vec(), Duration::from_millis(800)),
        ],
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
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
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
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
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
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
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
        &[
            ("COSH_SHELL_LANG", "en-US"),
            ("TERM", "xterm-256color"),
            ("COSH_SHELL_WIDTH", "54"),
        ],
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
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
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
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
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
        &[("COSH_SHELL_LANG", "en-US"), ("TERM", "xterm-256color")],
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
        &[
            ("COSH_SHELL_LANG", "en-US"),
            ("COSH_SHELL_RENDER", "plain"),
            ("TERM", "xterm-256color"),
        ],
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
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("NO_COLOR", "1"), ("TERM", "dumb")],
        vec![
            (b"ls /path/that/does/not/exist\n".to_vec(), Duration::ZERO),
            (b"exit 0\n".to_vec(), Duration::from_millis(500)),
        ],
    );

    assert_plain_blocks(&output);
}

#[test]
fn raw_cli_explicit_plain_render_mode_uses_plain_blocks() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_RENDER", "plain"), ("TERM", "xterm-256color")],
        vec![
            (b"ls /path/that/does/not/exist\n".to_vec(), Duration::ZERO),
            (b"exit 0\n".to_vec(), Duration::from_millis(500)),
        ],
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
