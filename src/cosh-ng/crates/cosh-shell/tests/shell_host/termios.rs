use super::*;

#[test]
fn transparent_bash_preserves_user_stty_modes() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-transparent-stty-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("transparent-stty-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::line("stty -echo"),
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line(stty_flag_probe(
                "-echo",
                "__ECHO_OFF__",
                "__ECHO_ON__",
                "stty echo",
            )),
            RawRelayAction::line("stty -isig"),
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line(stty_flag_probe(
                "-isig",
                "__ISIG_OFF__",
                "__ISIG_ON__",
                "stty isig",
            )),
            RawRelayAction::line("stty -icanon min 1 time 0"),
            RawRelayAction::wait(Duration::from_millis(200)),
            RawRelayAction::line(stty_flag_probe(
                "-icanon",
                "__ICANON_OFF__",
                "__ICANON_ON__",
                "stty icanon",
            )),
            RawRelayAction::line("stty sane"),
        ],
        &mut rendered,
    )
    .expect("raw relay stty parity");

    let ledger = ledger_from_output(&output);
    let command_output = ledger_output_refs_text(&ledger);
    assert!(command_output.contains("__ECHO_OFF__"), "{command_output}");
    assert!(!command_output.contains("__ECHO_ON__"), "{command_output}");
    assert!(command_output.contains("__ISIG_OFF__"), "{command_output}");
    assert!(!command_output.contains("__ISIG_ON__"), "{command_output}");
    assert!(
        command_output.contains("__ICANON_OFF__"),
        "{command_output}"
    );
    assert!(
        !command_output.contains("__ICANON_ON__"),
        "{command_output}"
    );
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("stty sane") && block.exit_code == 0));
}

#[test]
fn transparent_ctrl_d_exits_bash_and_zsh() {
    if Command::new("bash").arg("--version").output().is_ok() {
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-shell-bash-ctrl-d-test-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let config = ShellHostConfig::new("bash-ctrl-d-test", &work_dir);
        let mut rendered = Vec::new();
        let output = run_raw_relay_bash_with_actions(
            &config,
            vec![
                RawRelayAction::wait(Duration::from_millis(200)),
                RawRelayAction::write(vec![0x04]),
                RawRelayAction::wait(Duration::from_millis(300)),
                RawRelayAction::line("echo __BASH_AFTER_CTRL_D__"),
            ],
            &mut rendered,
        )
        .expect("bash ctrl-d");

        let rendered_text = String::from_utf8_lossy(&rendered);
        assert!(
            !rendered_text.contains("__BASH_AFTER_CTRL_D__"),
            "{rendered_text}"
        );
        assert!(output
            .events
            .iter()
            .any(|event| event.kind == ShellEventKind::ShellExited));
    }

    if Command::new("zsh").arg("--version").output().is_ok() {
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-shell-zsh-ctrl-d-test-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut config = ShellHostConfig::new("zsh-ctrl-d-test", &work_dir);
        config.native_mode = false;
        let mut rendered = Vec::new();
        let output = run_raw_relay_zsh_with_actions(
            &config,
            vec![
                RawRelayAction::wait(Duration::from_millis(200)),
                RawRelayAction::write(vec![0x04]),
                RawRelayAction::wait(Duration::from_millis(300)),
                RawRelayAction::line("echo __ZSH_AFTER_CTRL_D__"),
            ],
            &mut rendered,
        )
        .expect("zsh ctrl-d");

        let rendered_text = String::from_utf8_lossy(&rendered);
        assert!(
            !rendered_text.contains("__ZSH_AFTER_CTRL_D__"),
            "{rendered_text}"
        );
        assert!(output
            .events
            .iter()
            .any(|event| event.kind == ShellEventKind::ShellExited));
    }
}

#[test]
fn transparent_ctrl_backslash_is_not_synthesized_from_ctrl_c() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-ctrl-backslash-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("ctrl-backslash-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::line(
                "bash -c 'trap \"\" INT; trap \"exit 0\" QUIT; while IFS= read -r _; do :; done'",
            ),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::write(vec![0x03]),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("printf '%s\\n' __AFTER_CTRL_C__"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::write(vec![0x1c]),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("printf '%s\\n' __AFTER_QUIT__"),
        ],
        &mut rendered,
    )
    .expect("ctrl-c ctrl-backslash parity");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("__AFTER_QUIT__"), "{rendered_text}");
    assert_no_synthetic_terminal_restore_after_interrupt(&rendered);

    let ledger = ledger_from_output(&output);
    assert!(!ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("__AFTER_CTRL_C__")));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("__AFTER_QUIT__") && block.exit_code == 0));
}

#[test]
fn raw_relay_host_preserves_user_tty_mutation_after_interrupt() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-tty-restore-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-tty-restore-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::line("stty -echo; sleep 5"),
            RawRelayAction::wait(Duration::from_millis(250)),
            RawRelayAction::write(vec![0x03]),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line(
                "if stty -a | tr ' ;' '\\n\\n' | grep -qx -- '-echo'; then printf '%s\\n' __STATE_OFF__; stty echo; else printf '%s\\n' __STATE_ON__; fi",
            ),
            RawRelayAction::line("echo after-tty-restore"),
        ],
        &mut rendered,
    )
    .expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("__STATE_OFF__"), "{rendered_text}");
    assert!(!rendered_text.contains("__STATE_ON__"), "{rendered_text}");
    assert!(
        rendered_text.contains("after-tty-restore"),
        "{rendered_text}"
    );
    assert!(
        !rendered_text.contains("stty echo icanon"),
        "{rendered_text}"
    );
    assert_no_osc_marker(&rendered);
    assert_no_synthetic_terminal_restore_after_interrupt(&rendered);

    let ledger = ledger_from_output(&output);
    assert!(!ledger
        .blocks
        .iter()
        .any(|block| { block.command.contains("stty echo icanon") }));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| { block.command.contains("echo after-tty-restore") && block.exit_code == 0 }));
}

#[test]
fn cosh_owned_timeout_recovery_restores_pty_without_visible_command() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-cosh-owned-recovery-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("cosh-owned-recovery-test", &work_dir);
    let command = "stty -echo; sleep 5";
    let mut emitted = false;
    let mut interrupted = false;
    let mut command_started_at: Option<Instant> = None;
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(900)),
            RawRelayAction::line(stty_flag_probe(
                "-echo",
                "__COSH_RECOVERY_ECHO_OFF__",
                "__COSH_RECOVERY_ECHO_ON__",
                "stty echo",
            )),
            RawRelayAction::line("echo after-cosh-recovery"),
        ],
        &mut rendered,
        move |events, _| {
            if !emitted {
                emitted = true;
                let request = ShellHandoffRequest::new(
                    command,
                    format!("$ {command}"),
                    "validation",
                    "policy",
                    "approval-cosh-owned-recovery",
                    "run-cosh-owned-recovery",
                    1,
                )
                .expect("handoff request");
                return Ok(RawObserverAction::EmitToPty(request));
            }
            if command_started_at.is_none()
                && events.iter().any(|event| {
                    event.kind == ShellEventKind::CommandStarted
                        && event.command.as_deref() == Some(command)
                })
            {
                command_started_at = Some(Instant::now());
            }
            if !interrupted
                && command_started_at
                    .is_some_and(|started| started.elapsed() > Duration::from_millis(250))
            {
                interrupted = true;
                return Ok(RawObserverAction::InterruptForeground);
            }
            Ok(RawObserverAction::Continue)
        },
    )
    .expect("cosh-owned recovery");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(
        rendered_text.contains("after-cosh-recovery"),
        "{rendered_text}"
    );
    assert_no_synthetic_terminal_restore_after_interrupt(&rendered);

    let ledger = ledger_from_output(&output);
    let command_output = ledger_output_refs_text(&ledger);
    assert!(
        command_output.contains("__COSH_RECOVERY_ECHO_ON__"),
        "{command_output}"
    );
    assert!(
        !command_output.contains("__COSH_RECOVERY_ECHO_OFF__"),
        "{command_output}"
    );
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-cosh-recovery") && block.exit_code == 0));
}
