use super::*;

#[test]
fn raw_relay_bash_invalid_utf8_never_enters_event_provenance() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-invalid-utf8-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = with_raw_byte_readline(ShellHostConfig::new("invalid-utf8-test", &work_dir));
    config.native_mode = false;
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash(
        &config,
        std::io::Cursor::new(vec![0xff, b'\n', b'e', b'x', b'i', b't', b'\n']),
        &mut rendered,
    )
    .expect("invalid utf8 relay");

    assert!(!format!("{:?}", output.events).contains('\u{fffd}'));
    #[cfg(not(target_os = "linux"))]
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::CommandStarted
            && event
                .command
                .as_deref()
                .is_some_and(|command| command != "exit")
    }));
    #[cfg(target_os = "linux")]
    {
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::CommandStarted
                && event.command.as_deref() == Some("<redacted sensitive command>")
        }));
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::CommandRoutingObserved
                && event
                    .routing
                    .as_ref()
                    .is_some_and(|routing| routing.proven && routing.unsafe_input)
        }));
    }
}

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
fn raw_relay_bash_up_recalls_intercepted_slash_command() {
    let root = std::env::temp_dir().join(format!(
        "cosh-shell-bash-1718-recall-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home = root.join("home");
    let work_dir = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join(".bashrc"),
        "export HISTFILE=\"$HOME/.bash_history\"\n\
         export HISTSIZE=1000\n\
         shopt -s histappend\n",
    )
    .expect("bashrc");
    std::fs::write(home.join(".bash_history"), "echo prior-shell-cmd\n").expect("history");

    let mut config = ShellHostConfig::new("bash-1718-recall", &work_dir)
        .with_env("HOME", home.display().to_string());
    config.slash_via_shell = true;
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line("/skills detail xlsx"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::write(b"\x1b[A".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::write(b"\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("1718 recall relay");

    let rendered_text = String::from_utf8_lossy(&rendered);
    let intercept_count = output
        .events
        .iter()
        .filter(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("/skills detail xlsx")
                && event.component.as_deref() == Some("slash")
        })
        .count();
    let recalled_prior_shell_cmd = output.events.iter().any(|event| {
        event.kind == ShellEventKind::CommandStarted
            && event.command.as_deref() == Some("echo prior-shell-cmd")
    });
    // Issue #1718: Up right after a slash intercept recalls the slash
    // command (intercepted again through the shell marker), not the older
    // shell command from bash history.
    assert_eq!(intercept_count, 2, "{rendered_text}");
    assert!(!recalled_prior_shell_cmd, "{rendered_text}");
    // The routed line must never execute as a shell command.
    assert!(!rendered_text.contains("bash: /skills"), "{rendered_text}");

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn raw_relay_bash_routed_slash_enters_native_history_file() {
    let root = std::env::temp_dir().join(format!(
        "cosh-shell-bash-1718-histfile-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home = root.join("home");
    let work_dir = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join(".bashrc"),
        "export HISTFILE=\"$HOME/.bash_history\"\n\
         export HISTSIZE=1000\n\
         shopt -s histappend\n",
    )
    .expect("bashrc");

    let mut config = ShellHostConfig::new("bash-1718-histfile", &work_dir)
        .with_env("HOME", home.display().to_string());
    config.slash_via_shell = true;
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line("/skills detail xlsx"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("1718 histfile relay");

    let rendered_text = String::from_utf8_lossy(&rendered);
    let intercepted = output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/skills detail xlsx")
            && event.component.as_deref() == Some("slash")
    });
    assert!(intercepted, "{rendered_text}");
    // bash owns persistence: the routed slash reaches HISTFILE through the
    // user's native histappend semantics, with no cosh-side writes.
    let history = std::fs::read_to_string(home.join(".bash_history")).expect("histfile");
    assert!(history.contains("/skills detail xlsx"), "{history}");

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn raw_relay_bash_slash_route_switch_off_keeps_rust_intercept() {
    let root = std::env::temp_dir().join(format!(
        "cosh-shell-bash-1718-switch-off-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home = root.join("home");
    let work_dir = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join(".bashrc"),
        "export HISTFILE=\"$HOME/.bash_history\"\n\
         export HISTSIZE=1000\n\
         shopt -s histappend\n",
    )
    .expect("bashrc");
    std::fs::write(home.join(".bash_history"), "echo prior-shell-cmd\n").expect("history");

    let mut config = ShellHostConfig::new("bash-1718-switch-off", &work_dir)
        .with_env("HOME", home.display().to_string());
    config.slash_via_shell = false;
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line("/skills detail xlsx"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::write(b"\x1b[A".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::write(b"\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("1718 switch-off relay");

    let rendered_text = String::from_utf8_lossy(&rendered);
    let intercept_count = output
        .events
        .iter()
        .filter(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("/skills detail xlsx")
                && event.component.as_deref() == Some("slash")
        })
        .count();
    let recalled_prior_shell_cmd = output.events.iter().any(|event| {
        event.kind == ShellEventKind::CommandStarted
            && event.command.as_deref() == Some("echo prior-shell-cmd")
    });
    // COSH_SLASH_VIA_SHELL=0 restores the pre-#1718 chain end to end: the
    // slash is intercepted in the Rust relay, never enters history, and Up
    // recalls the older shell command.
    assert_eq!(intercept_count, 1, "{rendered_text}");
    assert!(recalled_prior_shell_cmd, "{rendered_text}");

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn raw_relay_bash_routed_slash_with_secret_never_persists_in_history() {
    let root = std::env::temp_dir().join(format!(
        "cosh-shell-bash-1718-secret-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home = root.join("home");
    let work_dir = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join(".bashrc"),
        "export HISTFILE=\"$HOME/.bash_history\"\n\
         export HISTSIZE=1000\n\
         shopt -s histappend\n",
    )
    .expect("bashrc");

    let mut config = ShellHostConfig::new("bash-1718-secret", &work_dir)
        .with_env("HOME", home.display().to_string());
    config.slash_via_shell = true;
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line("/config set api_key=sk-test-secret"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("1718 secret relay");

    let rendered_text = String::from_utf8_lossy(&rendered);
    let intercepted = output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("slash")
    });
    assert!(intercepted, "{rendered_text}");
    // The intercept branch scrubs credential-bearing entries before its
    // return 1, so the routed line must never reach the history file.
    let history = std::fs::read_to_string(home.join(".bash_history")).unwrap_or_default();
    assert!(!history.contains("api_key"), "{history}");

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn raw_relay_bash_intercepts_recalled_duplicate_slash_each_time() {
    let root = std::env::temp_dir().join(format!(
        "cosh-shell-bash-recalled-slash-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home = root.join("home");
    let work_dir = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join(".bashrc"),
        "export HISTFILE=\"$HOME/.bash_history\"\n\
         export HISTSIZE=1000\n\
         export HISTCONTROL=ignoredups\n\
         shopt -s histappend\n",
    )
    .expect("bashrc");
    std::fs::write(home.join(".bash_history"), "/skills detail xlsx\n").expect("history");

    let config = ShellHostConfig::new("bash-recalled-slash-test", &work_dir)
        .with_env("HOME", home.display().to_string());
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::write(b"\x1b[A".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::write(b"\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::write(b"\x1b[A".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::write(b"\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("recalled slash relay");

    let rendered_text = String::from_utf8_lossy(&rendered);
    let intercept_count = output
        .events
        .iter()
        .filter(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("/skills detail xlsx")
                && event.component.as_deref() == Some("slash")
        })
        .count();
    assert_eq!(intercept_count, 2, "{rendered_text}");
    assert!(
        !rendered_text.contains("bash: /skills: No such file or directory"),
        "{rendered_text}"
    );

    std::fs::remove_dir_all(root).expect("cleanup");
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

#[test]
fn raw_relay_capture_ack_discards_same_read_multiline_suffix() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-capture-drain-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("capture-drain-test", &work_dir);
    config.native_mode = false;
    let capture = RawInputCapture::Question {
        id: "question-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"yes\necho capture-drain-ok\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
        ],
        Vec::new(),
        move |events, _| {
            if events.iter().any(|event| {
                event.component.as_deref() == Some("card")
                    && event.message.as_deref() == Some("answer")
            }) {
                Ok(RawObserverAction::Continue)
            } else {
                Ok(RawObserverAction::CaptureInput(capture.clone()))
            }
        },
    )
    .expect("capture drain relay");

    let blocks: Vec<_> = ledger_from_output(&output)
        .blocks
        .into_iter()
        .filter(|block| block.command == "echo capture-drain-ok")
        .collect();
    assert!(blocks.is_empty(), "{:?}", output.events);
    assert!(output.events.iter().any(|event| {
        event.message.as_deref() == Some("capture_submitted")
            && event.capture.as_ref().is_some_and(|capture| {
                capture.kind.as_deref() == Some("question")
                    && capture.target_id.as_deref() == Some("question-1")
                    && capture.generation > 0
                    && capture.lifecycle == cosh_shell::types::ShellCaptureLifecycle::Submitted
            })
    }));
    assert!(output
        .events
        .iter()
        .any(|event| event.message.as_deref() == Some("capture_drained")));
}

#[test]
fn raw_relay_capture_chain_discards_old_generation_suffix() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-capture-chain-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("capture-chain-test", &work_dir);
    config.native_mode = false;
    let first = RawInputCapture::Question {
        id: "question-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let second = RawInputCapture::Question {
        id: "question-2".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"first\nsecond\necho capture-chain-ok\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"actual-second\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
        ],
        Vec::new(),
        move |events, _| {
            if events.iter().any(|event| {
                event.component.as_deref() == Some("card")
                    && event.message.as_deref() == Some("answer")
                    && event.input.as_deref() == Some("actual-second")
            }) {
                Ok(RawObserverAction::Continue)
            } else if events.iter().any(|event| {
                event.component.as_deref() == Some("card")
                    && event.message.as_deref() == Some("answer")
                    && event.input.as_deref() == Some("first")
            }) {
                Ok(RawObserverAction::CaptureInput(second.clone()))
            } else {
                Ok(RawObserverAction::CaptureInput(first.clone()))
            }
        },
    )
    .expect("capture chain relay");

    let blocks: Vec<_> = ledger_from_output(&output)
        .blocks
        .into_iter()
        .filter(|block| block.command == "echo capture-chain-ok")
        .collect();
    assert!(blocks.is_empty(), "{:?}", output.events);
    for answer in ["first", "actual-second"] {
        assert!(output.events.iter().any(|event| {
            event.message.as_deref() == Some("answer") && event.input.as_deref() == Some(answer)
        }));
    }
    assert!(!output.events.iter().any(|event| {
        event.message.as_deref() == Some("answer") && event.input.as_deref() == Some("second")
    }));
}

#[test]
fn raw_relay_capture_target_gone_discards_old_suffix_then_installs_new_target() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-capture-target-gone-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("capture-target-gone-test", &work_dir);
    config.native_mode = false;
    let first = RawInputCapture::Question {
        id: "question-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let second = RawInputCapture::Question {
        id: "question-2".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let third = RawInputCapture::Question {
        id: "question-3".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let mut calls_after_first_drain = 0;
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"first\necho abandoned-suffix-ok\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::write(b"answer-b\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::write(b"echo after-abandon-ok\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
        ],
        Vec::new(),
        move |events, _| {
            if events.iter().any(|event| {
                event.message.as_deref() == Some("answer")
                    && event.input.as_deref() == Some("answer-b")
            }) {
                Ok(RawObserverAction::Continue)
            } else if events
                .iter()
                .any(|event| event.message.as_deref() == Some("capture_drained"))
            {
                calls_after_first_drain += 1;
                if calls_after_first_drain == 1 {
                    Ok(RawObserverAction::Continue)
                } else {
                    Ok(RawObserverAction::CaptureInput(third.clone()))
                }
            } else if events.iter().any(|event| {
                event.message.as_deref() == Some("answer")
                    && event.input.as_deref() == Some("first")
            }) {
                Ok(RawObserverAction::CaptureInput(second.clone()))
            } else {
                Ok(RawObserverAction::CaptureInput(first.clone()))
            }
        },
    )
    .expect("capture target gone relay");

    let ledger = ledger_from_output(&output);
    assert_eq!(
        ledger
            .blocks
            .iter()
            .filter(|block| block.command == "echo abandoned-suffix-ok")
            .count(),
        0,
        "{:?}",
        output.events
    );
    assert_eq!(
        ledger
            .blocks
            .iter()
            .filter(|block| block.command == "echo after-abandon-ok")
            .count(),
        1,
        "{:?}",
        output.events
    );
    assert!(!output.events.iter().any(|event| {
        event.message.as_deref() == Some("answer")
            && event.input.as_deref() == Some("echo abandoned-suffix-ok")
    }));
    assert!(output.events.iter().any(|event| {
        event.message.as_deref() == Some("answer") && event.input.as_deref() == Some("answer-b")
    }));
}

#[test]
fn raw_relay_capture_eof_discards_old_generation_suffix() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-capture-eof-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("capture-eof-test", &work_dir);
    config.native_mode = false;
    let first = RawInputCapture::Question {
        id: "question-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let second = RawInputCapture::Question {
        id: "question-2".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"first\necho eof-suffix-ok\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
        ],
        Vec::new(),
        move |events, _| {
            if events
                .iter()
                .any(|event| event.message.as_deref() == Some("capture_drained"))
            {
                Ok(RawObserverAction::Continue)
            } else if events.iter().any(|event| {
                event.message.as_deref() == Some("answer")
                    && event.input.as_deref() == Some("first")
            }) {
                Ok(RawObserverAction::CaptureInput(second.clone()))
            } else {
                Ok(RawObserverAction::CaptureInput(first.clone()))
            }
        },
    )
    .expect("capture eof relay");

    assert_eq!(
        ledger_from_output(&output)
            .blocks
            .iter()
            .filter(|block| block.command == "echo eof-suffix-ok")
            .count(),
        0,
        "{:?}",
        output.events
    );
}

#[test]
fn raw_relay_capture_owned_input_overflow_is_visible_and_discarded() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-capture-overflow-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("capture-overflow-test", &work_dir);
    config.native_mode = false;
    let capture = RawInputCapture::Question {
        id: "question-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let mut input = b"yes\n#".to_vec();
    input.extend(std::iter::repeat_n(b'x', 64 * 1024));
    input.extend_from_slice(b"\necho capture-overflow-ok\n");
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(input),
            RawRelayAction::wait(Duration::from_millis(100)),
        ],
        Vec::new(),
        move |events, _| {
            if events
                .iter()
                .any(|event| event.message.as_deref() == Some("capture_overflow"))
            {
                Ok(RawObserverAction::Continue)
            } else {
                Ok(RawObserverAction::CaptureInput(capture.clone()))
            }
        },
    )
    .expect("capture overflow relay");

    let blocks: Vec<_> = ledger_from_output(&output)
        .blocks
        .into_iter()
        .filter(|block| block.command == "echo capture-overflow-ok")
        .collect();
    assert!(blocks.is_empty(), "{:?}", output.events);
    assert!(output
        .events
        .iter()
        .any(|event| event.message.as_deref() == Some("capture_overflow")));
}
