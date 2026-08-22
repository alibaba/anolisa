use super::*;

#[test]
fn native_integration_leaves_bash_hooks_and_input_owned_by_bash() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-native-integration-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(
        home_dir.join(".bashrc"),
        r#"PROMPT_COMMAND='printf "__USER_PROMPT_COMMAND__\n"'
trap 'printf "%s\n" "$BASH_COMMAND" >> "$HOME/debug-trap.log"' DEBUG
trap 'printf "%s\n" "$BASH_COMMAND" >> "$HOME/return-trap.log"' RETURN
trap 'printf "%s\n" "$BASH_COMMAND" >> "$HOME/err-trap.log"' ERR
"#,
    )
    .expect("bashrc");
    let config = ShellHostConfig::new("native-integration", &work_dir)
        .with_integration(ShellIntegration::Native)
        .with_env("HOME", home_dir.display().to_string())
        .with_env("PATH", "/usr/bin:/bin");
    assert_eq!(config.integration, ShellIntegration::Native);

    let mut rendered = Vec::new();
    let output = shell_run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line("printf '__FLAGS__=%s\\n' \"$-\""),
            RawRelayAction::line("shopt -q extdebug; printf '__EXTDEBUG_STATUS__=%s\\n' \"$?\""),
            RawRelayAction::line("set -o | { grep -E '^(errtrace|functrace)[[:space:]]' || :; }"),
            RawRelayAction::line("printf '__DEBUG_TRAP__=%q\\n' \"$(trap -p DEBUG)\""),
            RawRelayAction::line("printf '__COSH_SESSION_ID__=%s\\n' \"${COSH_SESSION_ID-unset}\""),
            RawRelayAction::line("set -x"),
            RawRelayAction::line("printf '__XTRACE_ALIVE__\\n'"),
            RawRelayAction::line("set +x"),
            RawRelayAction::line("hello"),
            RawRelayAction::line("/"),
            RawRelayAction::line("set -f; ??; set +f"),
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("native bash relay");

    let terminal = String::from_utf8_lossy(&rendered);
    let flags = terminal
        .lines()
        .find_map(|line| line.trim().strip_prefix("__FLAGS__="))
        .expect("shell flags");
    assert!(!flags.contains('E'), "{terminal}");
    assert!(!flags.contains('T'), "{terminal}");
    assert!(terminal.contains("__EXTDEBUG_STATUS__=1"), "{terminal}");
    assert!(
        terminal.contains("errtrace") && terminal.contains("off"),
        "{terminal}"
    );
    assert!(
        terminal.contains("functrace") && terminal.contains("off"),
        "{terminal}"
    );
    assert!(terminal.contains("__USER_PROMPT_COMMAND__"), "{terminal}");
    assert!(terminal.contains("__XTRACE_ALIVE__"), "{terminal}");
    assert!(terminal.contains("__COSH_SESSION_ID__=unset"), "{terminal}");
    assert!(!terminal.contains("_cosh"), "{terminal}");
    assert!(!terminal.contains("COSH_MARKER_TOKEN"), "{terminal}");
    assert!(!work_dir.join("cosh-marker.bash").exists());
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && matches!(event.input.as_deref(), Some("hello" | "/" | "??"))
    }));
    assert!(
        output.events.iter().all(|event| {
            matches!(
                event.kind,
                ShellEventKind::ShellStarted | ShellEventKind::ShellExited
            )
        }),
        "{:?}",
        output.events
    );

    for trap_log in ["debug-trap.log", "return-trap.log", "err-trap.log"] {
        let content = std::fs::read_to_string(home_dir.join(trap_log)).unwrap_or_default();
        assert!(!content.contains("_cosh"), "{trap_log}: {content}");
        assert!(
            !content.contains("COSH_MARKER_TOKEN"),
            "{trap_log}: {content}"
        );
    }

    let _ = std::fs::remove_dir_all(&work_dir);
}

#[test]
fn enhanced_assisted_integration_remains_the_default() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-enhanced-integration-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    let config = ShellHostConfig::new("enhanced-integration", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    assert_eq!(config.integration, ShellIntegration::Enhanced);
    let output = shell_run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("hello"),
            ScriptedInput::user_line("hello there"),
        ],
    )
    .expect("enhanced bash session");

    assert!(work_dir.join("cosh-marker.bash").is_file());
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("hello")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("hello there")
    }));

    let _ = std::fs::remove_dir_all(&work_dir);
}

#[test]
fn enhanced_shift_tab_toggles_shell_only_routing_without_restarting_bash() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-enhanced-toggle-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(home_dir.join(".bashrc"), "PS1='switch$ '\nKEEP_ME=alive\n").expect("bashrc");
    let config = ShellHostConfig::new("enhanced-toggle", &work_dir)
        .with_integration(ShellIntegration::Enhanced)
        .with_env("HOME", home_dir.display().to_string());

    let mut rendered = Vec::new();
    let output = shell_run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::write(b"\x1b[Z".to_vec()),
            RawRelayAction::line("hello there"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("/help"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("printf '__KEEP__=%s\\n' \"$KEEP_ME\""),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::write(b"\x1b[Z".to_vec()),
            RawRelayAction::line("hello there"),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("enhanced toggle relay");

    let terminal = String::from_utf8_lossy(&rendered);
    assert!(terminal.contains("\r\x1b[2K◌ switch$ "), "{terminal}");
    assert!(terminal.contains("\r\x1b[2K◇ switch$ "), "{terminal}");
    assert!(terminal.contains("__KEEP__=alive"), "{terminal}");
    let intercepted = output
        .events
        .iter()
        .filter(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("hello there")
        })
        .count();
    assert_eq!(intercepted, 1, "{:?}", output.events);
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/help")
    }));

    let _ = std::fs::remove_dir_all(&work_dir);
}

#[test]
fn native_integration_leaves_zsh_startup_and_input_owned_by_zsh() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-native-zsh-integration-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(home_dir.join(".zshrc"), "printf '__USER_ZSHRC__\\n'\n").expect("zshrc");
    let config = ShellHostConfig::new("native-zsh-integration", &work_dir)
        .with_integration(ShellIntegration::Native)
        .with_env("HOME", home_dir.display().to_string())
        .with_env("ZDOTDIR", home_dir.display().to_string());

    let mut rendered = Vec::new();
    let output = shell_run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line("hello"),
            RawRelayAction::line("/"),
            RawRelayAction::line("setopt NO_NOMATCH; ??"),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("native zsh relay");

    let terminal = String::from_utf8_lossy(&rendered);
    assert!(terminal.contains("__USER_ZSHRC__"), "{terminal}");
    assert!(!terminal.contains("_cosh"), "{terminal}");
    assert!(!work_dir.join(".zshrc").exists());
    assert!(output.events.iter().all(|event| {
        matches!(
            event.kind,
            ShellEventKind::ShellStarted | ShellEventKind::ShellExited
        )
    }));

    let _ = std::fs::remove_dir_all(&work_dir);
}

#[test]
fn line_interactive_host_routes_input_to_bash_and_journal() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-line-host-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("line-host-test", &work_dir);
    let input = std::io::Cursor::new(
        "/explain last error\n\
         echo line-ok\n\
         please explain the last error\n\
         ls /path/that/does/not/exist\n",
    );
    let mut rendered = Vec::new();
    let output =
        run_line_interactive_bash(&config, input, &mut rendered).expect("line interactive host");

    let rendered_text = String::from_utf8_lossy(&output.rendered_output);
    assert!(!rendered_text.contains("intercepted  slash"));
    assert!(!rendered_text.contains("intercepted  natural_language"));
    assert!(rendered_text.contains("line-ok"));

    let replayed_events = read_shell_events(&output.shell.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo line-ok") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("/path/that/does/not/exist") && block.exit_code != 0));
}

#[test]
fn line_interactive_host_can_invoke_claude_adapter_through_governance() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-line-claude-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("line-claude-test", &work_dir);
    let input = std::io::Cursor::new(
        "/explain last error\n\
         ls /path/that/does/not/exist\n",
    );
    let mut rendered = Vec::new();
    let output =
        run_line_interactive_bash(&config, input, &mut rendered).expect("line interactive host");

    let replayed_events = read_shell_events(&output.shell.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);

    let failed = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("/path/that/does/not/exist"))
        .expect("failed command block");
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("line-claude-test", failed, &findings, true)
        .expect("confirmed request");

    let agent_events = adapter_for_kind(AdapterKind::ClaudeCode)
        .run(&request)
        .expect("claude dry-run adapter");
    assert!(agent_events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. }
            if text.contains("Claude Code adapter prepared")
                && text.contains("--print")
    )));

    let governed = govern_agent_events(&agent_events, &Policy::default());
    assert!(governed.events.iter().all(|event| !event.auto_execute));
}

#[test]
fn line_interactive_host_runs_shell_command_with_non_ascii_path() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-line-unicode-path-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let file_name = "\u{8bbe}\u{8ba1}\u{6587}\u{6863}.md".to_string();
    let file_path = work_dir.join(&file_name);
    let file_content = "\u{4e2d}\u{6587}\u{5185}\u{5bb9}";
    std::fs::write(&file_path, file_content).expect("unicode file");

    let config = ShellHostConfig::new("line-unicode-path-test", &work_dir);
    let input = std::io::Cursor::new(format!("cat {}\necho after-cat\n", shell_arg(&file_path)));
    let mut rendered = Vec::new();
    let output =
        run_line_interactive_bash(&config, input, &mut rendered).expect("line interactive host");

    let rendered_text = String::from_utf8_lossy(&output.rendered_output);
    assert!(rendered_text.contains(file_content), "{rendered_text}");
    assert!(rendered_text.contains("after-cat"), "{rendered_text}");

    let replayed_events = read_shell_events(&output.shell.journal_path).expect("journal events");
    assert!(!replayed_events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("natural_language")
    }));

    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("cat ") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-cat") && block.exit_code == 0));
}
