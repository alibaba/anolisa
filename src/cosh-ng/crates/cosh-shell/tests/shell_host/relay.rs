use super::*;

#[test]
fn raw_relay_zsh_adapter_uses_shared_event_contract() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-raw-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let unicode_file = work_dir.join("\u{8bbe}\u{8ba1}\u{6587}\u{6863}.md");
    std::fs::write(&unicode_file, "\u{4e2d}\u{6587}\u{5185}\u{5bb9}").expect("unicode file");

    let config = ShellHostConfig::new("zsh-raw-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::line("/help"),
            RawRelayAction::line("echo zsh-raw-ok"),
            RawRelayAction::line(format!("cat {}", shell_arg(&unicode_file))),
            RawRelayAction::line("ls /path/that/does/not/exist"),
        ],
        &mut rendered,
    )
    .expect("raw zsh relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("zsh-raw-ok"), "{rendered_text}");
    assert!(
        rendered_text.contains("\u{4e2d}\u{6587}\u{5185}\u{5bb9}"),
        "{rendered_text}"
    );
    assert_no_osc_marker(&rendered);
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/help")
            && event.component.as_deref() == Some("slash")
    }));

    let ledger = ledger_from_output(&output);
    let echo_block = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("echo zsh-raw-ok") && block.exit_code == 0)
        .expect("zsh echo command block");
    assert_clean_shell_output_ref(echo_block, "zsh-raw-ok");
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("cat ") && block.exit_code == 0));
    assert!(ledger.blocks.iter().any(|block| {
        block.command.contains("/path/that/does/not/exist") && block.exit_code != 0
    }));
}

#[test]
fn raw_relay_zsh_buffers_fragmented_intercept_candidates() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-fragment-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    let config = ShellHostConfig::new("zsh-fragment-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::write("/he"),
            RawRelayAction::write("lp\n"),
            RawRelayAction::write("\u{4f60}".as_bytes()),
            RawRelayAction::write("\u{597d}\n".as_bytes()),
            RawRelayAction::write("?? zsh "),
            RawRelayAction::write("fragmented agent\n"),
            RawRelayAction::write("?? zsh combined agent\necho after-zsh-combined\n"),
            RawRelayAction::line("echo after-zsh-fragment"),
        ],
        &mut rendered,
    )
    .expect("raw zsh fragmented relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(
        rendered_text.contains("after-zsh-fragment"),
        "{rendered_text}"
    );
    assert!(
        rendered_text.contains("after-zsh-combined"),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("zsh: no such file or directory: /help"));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/help")
            && event.component.as_deref() == Some("slash")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("\u{4f60}\u{597d}")
            && event.component.as_deref() == Some("natural_language")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("?? zsh fragmented agent")
            && event.component.as_deref() == Some("agent_marker")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("?? zsh combined agent")
            && event.component.as_deref() == Some("agent_marker")
    }));
}

#[test]
fn raw_relay_bash_intercepts_fragmented_slash_while_typing() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-slash-completion-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    let config = ShellHostConfig::new("slash-completion-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(500)),
            RawRelayAction::write(b"/".to_vec()),
            RawRelayAction::wait(Duration::from_millis(150)),
            RawRelayAction::write(b"mo".to_vec()),
            RawRelayAction::wait(Duration::from_millis(150)),
            RawRelayAction::write(b"de approval auto\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(150)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
        |_, _| Ok(RawObserverAction::Continue),
    )
    .expect("raw bash slash completion");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("/"), "{rendered_text}");
    assert!(
        !rendered_text.contains("cosh-osc$ /  /help  /mode  /details  /skill"),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("/m/mo/mod/mode"), "{rendered_text}");
    assert!(
        output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("/mode approval auto")
                && event.component.as_deref() == Some("slash")
        }),
        "{rendered_text}\n{:?}",
        output.events
    );
    assert!(!rendered_text.contains("bash: /mode"), "{rendered_text}");
}

#[test]
fn raw_relay_zsh_preserves_session_history() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-history-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    let mut config = ShellHostConfig::new("zsh-history-test", &work_dir);
    config.native_mode = false;
    let mut rendered = Vec::new();
    run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::line("pwd"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("history"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("ls -ltrh"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("history"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("raw zsh history");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("    1  pwd"), "{rendered_text}");
    assert!(
        rendered_text.contains("    3  ls -ltrh") || rendered_text.contains("    2  ls -ltrh"),
        "{rendered_text}"
    );
}

#[test]
fn raw_relay_bash_excludes_secrets_from_history_and_journal() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-secret-history-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let history_snapshot = work_dir.join("history-snapshot");
    let secret = "history-secret-value";
    let access_key = "LTAI5tExampleAccessKey";
    let url_password = "history-url-password";
    let mut config = ShellHostConfig::new("bash-secret-history-test", &work_dir);
    config.native_mode = false;
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::line(format!("TOKEN={secret} true")),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line(format!(": {access_key}")),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line(format!(": https://user:{url_password}@example.test")),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line(format!("history > {}", shell_arg(&history_snapshot))),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
    )
    .expect("raw bash secret history");

    let history = std::fs::read_to_string(&history_snapshot).expect("history snapshot");
    let journal = std::fs::read_to_string(&output.journal_path).expect("journal");
    assert!(!history.contains(secret), "{history}");
    assert!(!history.contains(access_key), "{history}");
    assert!(!history.contains(url_password), "{history}");
    assert!(!journal.contains(secret), "{journal}");
    assert!(!journal.contains(access_key), "{journal}");
    assert!(!journal.contains(url_password), "{journal}");
    assert!(ledger_from_output(&output)
        .blocks
        .iter()
        .all(|block| !block.command.contains(secret)
            && !block.command.contains(access_key)
            && !block.command.contains(url_password)));
}

#[test]
fn raw_relay_zsh_excludes_secrets_from_history_and_journal() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-secret-history-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let history_snapshot = work_dir.join("history-snapshot");
    let secret = "history-secret-value";
    let access_key = "LTAI5tExampleAccessKey";
    let url_password = "history-url-password";
    let mut config = ShellHostConfig::new("zsh-secret-history-test", &work_dir);
    config.native_mode = false;
    let output = run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::line(format!("TOKEN={secret} true")),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line(format!(": {access_key}")),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line(format!(": https://user:{url_password}@example.test")),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line(format!("fc -l -100 > {}", shell_arg(&history_snapshot))),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
    )
    .expect("raw zsh secret history");

    let history = std::fs::read_to_string(&history_snapshot).expect("history snapshot");
    let journal = std::fs::read_to_string(&output.journal_path).expect("journal");
    assert!(!history.contains(secret), "{history}");
    assert!(!history.contains(access_key), "{history}");
    assert!(!history.contains(url_password), "{history}");
    assert!(!journal.contains(secret), "{journal}");
    assert!(!journal.contains(access_key), "{journal}");
    assert!(!journal.contains(url_password), "{journal}");
    assert!(ledger_from_output(&output)
        .blocks
        .iter()
        .all(|block| !block.command.contains(secret)
            && !block.command.contains(access_key)
            && !block.command.contains(url_password)));
}

#[test]
fn raw_relay_hold_mode_drops_input_without_writing_to_bash() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-hold-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("hold-test", &work_dir);
    let mut observer_calls = 0usize;
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("echo should-not-run"),
        ],
        Vec::new(),
        move |_, _| {
            observer_calls += 1;
            if observer_calls < 20 {
                Ok(RawObserverAction::HoldShellOutput)
            } else {
                Ok(RawObserverAction::Continue)
            }
        },
    )
    .expect("raw relay hold mode");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(!terminal.contains("should-not-run"), "{terminal}");
    let ledger = ledger_from_output(&output);
    assert!(!ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("should-not-run")));
}

#[test]
fn raw_relay_hold_mode_still_observes_ctrl_c() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-hold-ctrl-c-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("hold-ctrl-c-test", &work_dir);
    let mut observer_calls = 0usize;
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(vec![0x03]),
        ],
        Vec::new(),
        move |_, _| {
            observer_calls += 1;
            if observer_calls < 20 {
                Ok(RawObserverAction::HoldShellOutput)
            } else {
                Ok(RawObserverAction::Continue)
            }
        },
    )
    .expect("raw relay hold ctrl-c");

    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("control")
            && event.input.as_deref() == Some("ctrl_c")
    }));
}
