use super::*;

struct TemporaryWorkDir(std::path::PathBuf);

impl Drop for TemporaryWorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn shell_host_bash_tracks_native_history_file_changes() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-history-file-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _cleanup = TemporaryWorkDir(work_dir.clone());
    let home_dir = work_dir.join("home");
    let relative_one = work_dir.join("relative-one");
    let relative_two = work_dir.join("relative-two");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::create_dir_all(&relative_one).expect("first relative dir");
    std::fs::create_dir_all(&relative_two).expect("second relative dir");

    let initial_history = home_dir.join("initial-history");
    let alternate_history = home_dir.join("alternate-history");
    std::fs::write(
        home_dir.join(".bashrc"),
        format!("export HISTFILE={}\n", shell_arg(&initial_history)),
    )
    .expect("bashrc");

    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observer = std::sync::Arc::clone(&observed);
    let config = ShellHostConfig::new("history-file-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string())
        .with_history_file_observer(move |path| {
            observer.lock().expect("history observer lock").push(path);
        });
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("echo unchanged-history-file"),
            ScriptedInput::user_line(format!("export HISTFILE={}", shell_arg(&alternate_history))),
            ScriptedInput::user_line("echo unchanged-alternate-history-file"),
            ScriptedInput::user_line(format!(
                "cd {}; export HISTFILE=relative-history",
                shell_arg(&relative_one)
            )),
            ScriptedInput::user_line(format!("cd {}", shell_arg(&relative_two))),
            ScriptedInput::user_line("false"),
        ],
    )
    .expect("scripted bash pty");

    assert_eq!(
        *observed.lock().expect("history observer lock"),
        vec![
            initial_history,
            alternate_history,
            relative_one.join("relative-history"),
            relative_two.join("relative-history"),
        ]
    );

    let ledger = ledger_from_output(&output);
    let failed = ledger
        .blocks
        .iter()
        .find(|block| block.command == "false")
        .expect("false command block");
    assert_eq!(failed.exit_code, 1);
}

#[test]
fn shell_host_bash_isolated_mode_omits_history_file_markers() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-isolated-history-file-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _cleanup = TemporaryWorkDir(work_dir.clone());
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observer = std::sync::Arc::clone(&observed);
    let mut config = ShellHostConfig::new("isolated-history-file-test", &work_dir)
        .with_history_file_observer(move |path| {
            observer.lock().expect("history observer lock").push(path);
        });
    config.native_mode = false;

    run_scripted_bash(&config, &[ScriptedInput::user_line("echo isolated")])
        .expect("scripted isolated bash pty");

    assert!(observed.lock().expect("history observer lock").is_empty());
}

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
    assert!(failed.shell_environment_generation.is_some());
    let output_ref = failed
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_ref_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(output_ref_text.contains("No such file") || output_ref_text.contains("cannot access"));
}

#[test]
fn shell_host_owns_prompt_boundary_before_user_prompt_command() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-command-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(home_dir.join(".bash_history"), "exit\n").expect("history");
    std::fs::write(
        home_dir.join(".bashrc"),
        "set -o history\n\
         HISTFILE=\"$HOME/.bash_history\"\n\
         history -r \"$HISTFILE\" 2>/dev/null || true\n\
         PROMPT_COMMAND='PATH=\"/prompt-hook:$PATH\"; history 1 >/dev/null; printf \"__cosh_prompt_noise__\\n\" >&2'\n",
    )
    .expect("bashrc");

    let config = ShellHostConfig::new("prompt-command-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[ScriptedInput::user_line("ls /path/that/does/not/exist")],
    )
    .expect("scripted bash pty");

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    let failed = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("/path/that/does/not/exist"))
        .expect("failed command block");
    assert_ne!(failed.exit_code, 0);
    assert_eq!(failed.shell_environment_generation, Some(2));
    let output_ref = failed
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_ref_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(
        !output_ref_text.contains("__cosh_prompt_noise__"),
        "{output_ref_text}"
    );
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
    assert!(ledger
        .blocks
        .iter()
        .filter(|block| block.command.contains("zsh-ok") || block.command.contains("cat "))
        .all(|block| block.shell_environment_generation.is_some()));
}

#[test]
fn shell_host_zsh_later_preexec_hook_fails_closed_for_path_generation() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-path-trust-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let config = ShellHostConfig::new("zsh-path-trust-test", &work_dir);
    let output = run_scripted_zsh(
        &config,
        &[
            ScriptedInput::user_line("function _cosh_test_later_preexec { PATH=/later:$PATH }"),
            ScriptedInput::user_line("add-zsh-hook preexec _cosh_test_later_preexec"),
            ScriptedInput::user_line("echo after-later-hook"),
        ],
    )
    .expect("scripted zsh pty");

    let ledger = ledger_from_output(&output);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command == "echo after-later-hook")
        .expect("command after later preexec hook");
    assert_eq!(block.shell_environment_generation, None);
}

#[test]
fn shell_host_bash_combined_debug_trap_fails_closed_for_path_generation() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-path-trust-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let config = ShellHostConfig::new("bash-path-trust-test", &work_dir);
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("trap '_cosh_preexec_marker; :' DEBUG"),
            ScriptedInput::user_line("echo after-combined-trap"),
        ],
    )
    .expect("scripted bash pty");

    let ledger = ledger_from_output(&output);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command == "echo after-combined-trap")
        .expect("command after combined DEBUG trap");
    assert_eq!(block.shell_environment_generation, None);
}

#[test]
fn shell_host_bash_captured_debug_trap_keeps_path_generation_trusted() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-captured-trap-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(
        home_dir.join(".bashrc"),
        "trap 'PATH=/captured:$PATH' DEBUG\n",
    )
    .expect("bashrc");
    let config = ShellHostConfig::new("bash-captured-trap-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[ScriptedInput::user_line("echo after-captured-trap")],
    )
    .expect("scripted bash pty");

    let ledger = ledger_from_output(&output);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command == "echo after-captured-trap")
        .expect("command after captured DEBUG trap");
    assert!(block.shell_environment_generation.is_some());
}
