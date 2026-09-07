use std::os::unix::process::ExitStatusExt;

use super::*;

#[test]
fn raw_cli_double_dash_passthrough_executes_command_directly() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
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
    let output = raw_cli_command(binary)
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
fn raw_cli_double_dash_passthrough_preserves_signal_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");

    for (signal, expected) in [("INT", 130), ("TERM", 143), ("KILL", 137)] {
        let command = format!("kill -{signal} $$");
        let output = raw_cli_command(binary)
            .args(["--", "sh", "-c", command.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run direct command terminated by signal");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(expected),
            "signal={signal}\nstdout={stdout}\nstderr={stderr}"
        );
    }
}

#[test]
fn raw_cli_double_dash_passthrough_preserves_start_failure_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .args(["--", "/definitely/not/a/cosh-shell-command"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run missing direct command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(126),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stderr.contains("exec /definitely/not/a/cosh-shell-command failed"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_double_dash_passthrough_does_not_capture_child_help_arg() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .args(["--", "printf", "%s\n", "--help"])
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
    let output = raw_cli_command(binary)
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
    let output = raw_cli_command(binary)
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
fn raw_cli_dash_c_passthrough_preserves_signal_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");

    // The passthrough execs the shell in place, so a signal death is
    // observable as a signal death (bash parity), not a 128+n exit code.
    for (signal, expected) in [("INT", 2), ("TERM", 15), ("KILL", 9)] {
        let command = format!("kill -{signal} $$");
        let output = raw_cli_command(binary)
            .args(["-c", command.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run dash-c command terminated by signal");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.signal(),
            Some(expected),
            "signal={signal}\nstdout={stdout}\nstderr={stderr}"
        );
    }
}

#[test]
fn raw_cli_dash_c_passthrough_filters_wrapper_shell_option() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
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
fn raw_cli_raw_adapter_dash_c_passthrough_executes_without_agent_ui() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .args(["raw", "cosh-core", "-c", "echo raw-adapter-c-ok"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run raw adapter dash-c passthrough");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("raw-adapter-c-ok"),
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
fn raw_cli_raw_adapter_dash_c_passthrough_preserves_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .args(["raw", "cosh-core", "-c", "exit 48"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run raw adapter dash-c passthrough with nonzero exit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(48),
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
fn raw_cli_stdin_passthrough_preserves_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let mut child = raw_cli_command(binary)
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
fn raw_cli_stdin_passthrough_preserves_signal_exit_status() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");

    // Same exec-in-place parity as the dash-c variant: the shell's signal
    // death reaches the caller as a signal, exactly like invoking bash.
    for (signal, expected) in [("INT", 2), ("TERM", 15), ("KILL", 9)] {
        let mut child = raw_cli_command(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn stdin passthrough");

        {
            let mut stdin = child.stdin.take().expect("child stdin");
            writeln!(stdin, "kill -{signal} $$").expect("write stdin command terminated by signal");
        }

        let output = child.wait_with_output().expect("wait stdin passthrough");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.signal(),
            Some(expected),
            "signal={signal}\nstdout={stdout}\nstderr={stderr}"
        );
    }
}

#[test]
fn raw_cli_login_dash_c_passthrough_executes_without_agent_ui() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
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
    let output = raw_cli_command(binary)
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
    let output = raw_cli_command(binary)
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
    let output = raw_cli_command(binary)
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
    let mut child = raw_cli_command(binary)
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
fn raw_cli_cosh_entry_combined_login_flags_reach_bash_with_arg0() {
    // /usr/bin/cosh contract: `-lc` is handed to bash verbatim and `$0`
    // reflects the invocation name, not a hardcoded shell name.
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .arg0("cosh")
        .args(["-lc", "printf '[%s]' \"$0\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run cosh entry with combined login flags");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("[cosh]"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_cosh_entry_login_argv0_reaches_inner_shell_dollar_zero() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .arg0("-cosh")
        .args(["-c", "printf '[%s]' \"$0\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run login cosh entry");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("[-cosh]"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_cosh_entry_invalid_option_is_judged_by_bash() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .arg0("cosh")
        .arg("--definitely-invalid")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run cosh entry with invalid option");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("Thinking..."),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stderr.contains("usage: cosh-shell"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_cosh_entry_missing_script_file_reports_bash_127() {
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .arg0("cosh")
        .arg("/definitely/not/present")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run cosh entry with missing script operand");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(127),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn raw_cli_cosh_entry_tui_only_flag_fails_loud_on_exec_path() {
    // TUI-only flags reach the inner shell verbatim on the exec path, so
    // their semantics are rejected loudly (bash: invalid option, exit 2)
    // instead of being silently dropped.
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .arg0("cosh")
        .args(["--isolated", "-c", "printf __SHOULD_NOT_RUN__"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run cosh entry with --isolated on the exec path");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stderr.contains("--isolated"),
        "stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("__SHOULD_NOT_RUN__"),
        "stdout={stdout}\nstderr={stderr}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn raw_cli_passthrough_preserves_ignored_sigpipe_disposition() {
    // An ignored SIGPIPE inherited by the cosh entry must reach the inner
    // shell (captured before the Rust runtime rewrite, restored in
    // pre_exec); the default disposition must stay default. SIGPIPE is
    // signal 13, so bit 13 of SigIgn is mask 0x1000.
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let probe = "grep SigIgn /proc/self/status";

    let sigign_mask = |ignore_pipe: bool| -> u64 {
        let prefix = if ignore_pipe { "trap '' PIPE; " } else { "" };
        let script = format!("{prefix}exec -a cosh '{binary}' -c '{probe}'");
        let output = raw_cli_command("bash")
            .args(["-c", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run sigpipe disposition probe");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
        let hex = stdout
            .split_whitespace()
            .nth(1)
            .unwrap_or_else(|| panic!("no SigIgn value in {stdout:?}"));
        u64::from_str_radix(hex, 16).expect("parse SigIgn mask")
    };

    assert_ne!(
        sigign_mask(true) & 0x1000,
        0,
        "ignored SIGPIPE must be inherited by the inner shell"
    );
    assert_eq!(
        sigign_mask(false) & 0x1000,
        0,
        "default SIGPIPE must stay default in the inner shell"
    );
}

#[test]
fn raw_cli_interactive_dash_c_passthrough_transports_env_ps1() {
    // Value-level prompt contract: env PS1 survives the compiled entry and
    // is visible to an interactive inner bash (non-interactive bash strips
    // it natively on both the oracle and candidate sides).
    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let output = raw_cli_command(binary)
        .arg0("cosh")
        .env("PS1", "__COSH_PS1_PROBE__")
        .args([
            "--norc",
            "--noprofile",
            "-i",
            "-c",
            "printf '[%s]' \"$PS1\"",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run interactive dash-c with env PS1");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("[__COSH_PS1_PROBE__]"),
        "stdout={stdout}\nstderr={stderr}"
    );
}
