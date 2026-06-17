use super::*;

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
