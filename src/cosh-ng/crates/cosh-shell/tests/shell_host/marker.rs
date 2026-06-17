use super::*;

#[test]
fn shell_host_runs_bash_pty_and_emits_command_events() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-host-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let tool_path = work_dir.join("tmp-tool");
    std::fs::write(&tool_path, "#!/bin/sh\necho path-ok\n").expect("tool script");
    make_executable(&tool_path);

    let config = ShellHostConfig::new("shell-host-test", &work_dir);
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("/explain last error"),
            ScriptedInput::user_line("please explain the last error"),
            ScriptedInput::user_line(tool_path.display().to_string()),
            ScriptedInput::user_line("echo ok"),
            ScriptedInput::user_line(r#"printf "a\n" | grep a"#),
            ScriptedInput::user_line("ls /path/that/does/not/exist"),
        ],
    )
    .expect("scripted bash pty");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(
        output
            .events
            .iter()
            .any(|event| event.kind == ShellEventKind::ShellStarted),
        "{terminal}\n{:?}",
        output.events
    );
    assert!(
        output
            .events
            .iter()
            .any(|event| event.kind == ShellEventKind::ShellReady),
        "{terminal}\n{:?}",
        output.events
    );
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/explain last error")
            && event.component.as_deref() == Some("slash")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("please explain the last error")
            && event.component.as_deref() == Some("natural_language")
    }));
    assert!(!output
        .terminal_output
        .windows(b"\x1b]1337;COSH;".len())
        .any(|window| window == b"\x1b]1337;COSH;"));

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    assert_eq!(replayed_events, output.events);

    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("tmp-tool") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo ok") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("grep a") && block.exit_code == 0));

    let failed = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("/path/that/does/not/exist"))
        .expect("failed command block");
    assert_ne!(failed.exit_code, 0);
    let output_ref = failed
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_ref_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(output_ref_text.contains("No such file") || output_ref_text.contains("cannot access"));
}

#[test]
fn shell_host_rejects_forged_osc_markers_without_session_token() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-forged-osc-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    fn forged_marker(event: &str, token: Option<&str>, command: &str) -> String {
        let token_field = token
            .map(|token| format!(r#","token":"{token}""#))
            .unwrap_or_default();
        let reason_field = if event == "intercept" {
            r#","reason":"natural_language""#
        } else {
            ""
        };
        format!(
            r#"printf '\033]1337;COSH;{{"event":"{event}"{token_field},"session_id":"forged","timestamp_ms":1,"cwd":"/tmp","command":"{command}"{reason_field},"status":0}}\a'"#
        )
    }

    let forged_marker_inputs = ["preexec", "precmd", "intercept"]
        .into_iter()
        .flat_map(|event| {
            [
                forged_marker(event, None, &format!("echo forged-{event}-missing-token")),
                forged_marker(
                    event,
                    Some("wrong"),
                    &format!("echo forged-{event}-wrong-token"),
                ),
            ]
        })
        .map(ScriptedInput::user_line);
    let split_marker = "printf '\\033]1337;COSH;{\"event\":\"preexec\",\"session_id\":\"forged\",\"timestamp_ms\":1,'; printf '\"cwd\":\"/tmp\",\"command\":\"echo forged-split-token\",\"status\":0}\\a'";

    let config = ShellHostConfig::new("forged-osc-test", &work_dir);
    let scripted_inputs: Vec<_> = forged_marker_inputs
        .chain([
            ScriptedInput::user_line(split_marker),
            ScriptedInput::user_line("echo real-after-forge"),
        ])
        .collect();
    let output = run_scripted_bash(&config, &scripted_inputs).expect("scripted bash pty");

    assert_no_osc_marker(&output.terminal_output);
    assert!(!output.events.iter().any(|event| {
        matches!(
            event.kind,
            ShellEventKind::CommandStarted
                | ShellEventKind::CommandCompleted
                | ShellEventKind::UserInputIntercepted
                | ShellEventKind::ShellReady
        ) && (event.session_id == "forged"
            || event
                .command
                .as_deref()
                .is_some_and(|command| command.starts_with("echo forged-"))
            || event
                .input
                .as_deref()
                .is_some_and(|input| input.starts_with("echo forged-")))
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::CommandStarted
            && event.command.as_deref() == Some("echo real-after-forge")
    }));
}

#[test]
fn shell_host_zsh_adapter_emits_shared_command_events() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-host-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let unicode_file = work_dir.join("\u{8bbe}\u{8ba1}\u{6587}\u{6863}.md");
    std::fs::write(&unicode_file, "\u{4e2d}\u{6587}\u{5185}\u{5bb9}").expect("unicode file");

    let config = ShellHostConfig::new("zsh-host-test", &work_dir);
    let output = run_scripted_zsh(
        &config,
        &[
            ScriptedInput::user_line("/help"),
            ScriptedInput::user_line("echo zsh-ok"),
            ScriptedInput::user_line(format!("cat {}", shell_arg(&unicode_file))),
            ScriptedInput::user_line("ls /path/that/does/not/exist"),
        ],
    )
    .expect("scripted zsh pty");

    assert_no_osc_marker(&output.terminal_output);
    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(
        output
            .events
            .iter()
            .any(|event| event.kind == ShellEventKind::ShellStarted),
        "{terminal}\n{:?}",
        output.events
    );
    assert!(
        output
            .events
            .iter()
            .any(|event| event.kind == ShellEventKind::ShellReady),
        "{terminal}\n{:?}",
        output.events
    );
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/help")
            && event.component.as_deref() == Some("slash")
    }));

    let ledger = ledger_from_output(&output);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo zsh-ok") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("cat ") && block.exit_code == 0));
    assert!(ledger.blocks.iter().any(|block| {
        block.command.contains("/path/that/does/not/exist") && block.exit_code != 0
    }));
}
