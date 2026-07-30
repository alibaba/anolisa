use super::*;

#[test]
fn raw_relay_approved_handoff_wrapper_does_not_leak_to_output() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-handoff-wrapper-leak-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("handoff-wrapper-leak-test", &work_dir);
    let mut emitted = false;
    let command = "printf handoff-visible";
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(500)),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
        move |_, _| {
            if emitted {
                return Ok(RawObserverAction::Continue);
            }
            emitted = true;
            let request = ShellHandoffRequest::new(
                command,
                format!("$ {command}"),
                "approved_provider_shell_tool",
                "user",
                "approval-1",
                "run-1",
                1,
            )
            .expect("handoff request");
            Ok(RawObserverAction::EmitToPty(request))
        },
    )
    .expect("raw relay handoff");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(terminal.contains("handoff-visible"), "{terminal}");
    assert!(
        !terminal.contains("COSH_SHELL_HANDOFF_BYPASS"),
        "{terminal}"
    );

    let ledger = ledger_from_output(&output);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command == command)
        .expect("original handoff command block");
    assert_eq!(block.exit_code, 0, "{terminal}");
    assert_clean_shell_output_ref(block, "handoff-visible");
    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(
        !output_text.contains("COSH_SHELL_HANDOFF_BYPASS"),
        "{output_text}"
    );
}

#[test]
fn raw_relay_handoff_provenance_does_not_set_child_environment() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-handoff-env-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("handoff-env-test", &work_dir);
    let mut emitted = false;
    let command = "sh -c 'printf \"handoff-bypass=%s\\n\" \"${COSH_SHELL_HANDOFF_BYPASS-unset}\"'";
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(500)),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
        move |_, _| {
            if emitted {
                return Ok(RawObserverAction::Continue);
            }
            emitted = true;
            let request = ShellHandoffRequest::new(
                command,
                format!("$ {command}"),
                "approved_provider_shell_tool",
                "user",
                "approval-env",
                "run-env",
                1,
            )
            .expect("handoff request");
            Ok(RawObserverAction::EmitToPty(request))
        },
    )
    .expect("raw relay handoff env");

    let ledger = ledger_from_output(&output);
    let command_output = ledger_output_refs_text(&ledger);
    assert!(
        command_output.contains("handoff-bypass=unset"),
        "{command_output}"
    );
    assert!(
        !command_output.contains("handoff-bypass=1"),
        "{command_output}"
    );
}

#[test]
fn raw_relay_zsh_approved_handoff_wrapper_does_not_leak_to_output() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-handoff-wrapper-leak-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("zsh-handoff-wrapper-leak-test", &work_dir);
    config.native_mode = false;
    let input = DelayedInput::new(vec![(b"exit\n".to_vec(), Duration::from_millis(700))]);
    let mut emitted = false;
    let command = "printf zsh-handoff-visible";
    let output = run_raw_relay_zsh_with_output_control(&config, input, Vec::new(), move |_, _| {
        if emitted {
            return Ok(RawObserverAction::Continue);
        }
        emitted = true;
        let request = ShellHandoffRequest::new(
            command,
            format!("$ {command}"),
            "approved_provider_shell_tool",
            "user",
            "approval-1",
            "run-1",
            1,
        )
        .expect("handoff request");
        Ok(RawObserverAction::EmitToPty(request))
    })
    .expect("raw zsh relay handoff");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(terminal.contains("zsh-handoff-visible"), "{terminal}");
    assert!(
        !terminal.contains("COSH_SHELL_HANDOFF_BYPASS"),
        "{terminal}"
    );

    let ledger = ledger_from_output(&output);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command == command)
        .expect("original zsh handoff command block");
    assert_eq!(block.exit_code, 0, "{terminal}");
    assert_clean_shell_output_ref(block, "zsh-handoff-visible");
    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(
        !output_text.contains("COSH_SHELL_HANDOFF_BYPASS"),
        "{output_text}"
    );
}

#[test]
fn raw_relay_bash_history_records_original_handoff_command() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-handoff-history-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("bash-handoff-history-test", &work_dir);
    config.native_mode = false;
    let mut emitted = false;
    let command = "printf bash-history-visible";
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(500)),
            RawRelayAction::line("history"),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
        move |_, _| {
            if emitted {
                return Ok(RawObserverAction::Continue);
            }
            emitted = true;
            let request = ShellHandoffRequest::new(
                command,
                format!("$ {command}"),
                "approved_provider_shell_tool",
                "user",
                "approval-1",
                "run-1",
                1,
            )
            .expect("handoff request");
            Ok(RawObserverAction::EmitToPty(request))
        },
    )
    .expect("raw bash handoff history");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(terminal.contains(command), "{terminal}");
    assert!(
        !terminal.contains("COSH_SHELL_HANDOFF_BYPASS"),
        "{terminal}"
    );
}

#[test]
fn raw_relay_zsh_history_records_original_handoff_command() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-handoff-history-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("zsh-handoff-history-test", &work_dir);
    config.native_mode = false;
    let input = DelayedInput::new(vec![
        (b"history\n".to_vec(), Duration::from_millis(700)),
        (b"exit\n".to_vec(), Duration::from_millis(100)),
    ]);
    let mut emitted = false;
    let command = "printf zsh-history-visible";
    let output = run_raw_relay_zsh_with_output_control(&config, input, Vec::new(), move |_, _| {
        if emitted {
            return Ok(RawObserverAction::Continue);
        }
        emitted = true;
        let request = ShellHandoffRequest::new(
            command,
            format!("$ {command}"),
            "approved_provider_shell_tool",
            "user",
            "approval-1",
            "run-1",
            1,
        )
        .expect("handoff request");
        Ok(RawObserverAction::EmitToPty(request))
    })
    .expect("raw zsh handoff history");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(terminal.contains(command), "{terminal}");
    assert!(
        !terminal.contains("COSH_SHELL_HANDOFF_BYPASS"),
        "{terminal}"
    );
}

fn pager_guard_handoff_request(command: &str, source: &str) -> ShellHandoffRequest {
    ShellHandoffRequest::new(
        command,
        format!("$ {command}"),
        source,
        "user",
        "approval-pg",
        "run-pg",
        1,
    )
    .expect("handoff request")
}

// Issue #1988: agent-approved handoff commands run with the pager family
// forced to cat so pager-capable tools cannot stall the turn on the
// foreground TTY; the guard must restore the user's environment exactly
// afterwards and must not touch user-initiated executions.
#[test]
fn raw_relay_bash_pager_guard_exports_cat_for_agent_handoff_and_restores() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-pager-guard-restore-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("pager-guard-restore-test", &work_dir);
    config
        .env_overrides
        .push(("GIT_PAGER".to_string(), "less".to_string()));
    let mut emitted = false;
    let command =
        "printf 'during=%s,%s,%s,%s\\n' \"${GIT_PAGER-unset}\" \"${PAGER-unset}\" \"${SYSTEMD_PAGER-unset}\" \"${MANPAGER-unset}\"";
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(700)),
            RawRelayAction::line(
                "printf 'after=%s,%s\\n' \"${GIT_PAGER-unset}\" \"${PAGER-unset}\"",
            ),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
        move |_, _| {
            if emitted {
                return Ok(RawObserverAction::Continue);
            }
            emitted = true;
            Ok(RawObserverAction::EmitToPty(pager_guard_handoff_request(
                command,
                "approved_provider_shell_tool",
            )))
        },
    )
    .expect("raw relay pager guard restore");

    let ledger = ledger_from_output(&output);
    let command_output = ledger_output_refs_text(&ledger);
    assert!(
        command_output.contains("during=cat,cat,cat,cat"),
        "{command_output}"
    );
    assert!(
        command_output.contains("after=less,unset"),
        "{command_output}"
    );
}

#[test]
fn raw_relay_bash_pager_guard_covers_compound_handoff_command() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-pager-guard-compound-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("pager-guard-compound-test", &work_dir);
    let mut emitted = false;
    let command = "true && printf 'compound=%s\\n' \"${GIT_PAGER-unset}\"";
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(700)),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
        move |_, _| {
            if emitted {
                return Ok(RawObserverAction::Continue);
            }
            emitted = true;
            Ok(RawObserverAction::EmitToPty(pager_guard_handoff_request(
                command,
                "approved_provider_shell_tool",
            )))
        },
    )
    .expect("raw relay pager guard compound");

    let ledger = ledger_from_output(&output);
    let command_output = ledger_output_refs_text(&ledger);
    assert!(command_output.contains("compound=cat"), "{command_output}");
}

#[test]
fn raw_relay_bash_pager_guard_skips_user_send_to_shell_handoff() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-pager-guard-send-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("pager-guard-send-test", &work_dir);
    let mut emitted = false;
    let command = "printf 'send=%s\\n' \"${GIT_PAGER-unset}\"";
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(700)),
            RawRelayAction::line("exit"),
        ],
        Vec::new(),
        move |_, _| {
            if emitted {
                return Ok(RawObserverAction::Continue);
            }
            emitted = true;
            Ok(RawObserverAction::EmitToPty(pager_guard_handoff_request(
                command,
                "send_to_shell",
            )))
        },
    )
    .expect("raw relay pager guard send_to_shell");

    let ledger = ledger_from_output(&output);
    let command_output = ledger_output_refs_text(&ledger);
    assert!(command_output.contains("send=unset"), "{command_output}");
}

#[test]
fn raw_relay_zsh_pager_guard_exports_cat_for_agent_handoff_and_restores() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-pager-guard-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("zsh-pager-guard-test", &work_dir);
    config.native_mode = false;
    config
        .env_overrides
        .push(("GIT_PAGER".to_string(), "less".to_string()));
    let input = DelayedInput::new(vec![
        (
            b"printf 'after=%s,%s\\n' \"${GIT_PAGER-unset}\" \"${PAGER-unset}\"\n".to_vec(),
            Duration::from_millis(900),
        ),
        (b"exit\n".to_vec(), Duration::from_millis(300)),
    ]);
    let mut emitted = false;
    let command = "printf 'during=%s,%s\\n' \"${GIT_PAGER-unset}\" \"${PAGER-unset}\"";
    let output = run_raw_relay_zsh_with_output_control(&config, input, Vec::new(), move |_, _| {
        if emitted {
            return Ok(RawObserverAction::Continue);
        }
        emitted = true;
        Ok(RawObserverAction::EmitToPty(pager_guard_handoff_request(
            command,
            "approved_provider_shell_tool",
        )))
    })
    .expect("raw zsh relay pager guard");

    let ledger = ledger_from_output(&output);
    let command_output = ledger_output_refs_text(&ledger);
    assert!(
        command_output.contains("during=cat,cat"),
        "{command_output}"
    );
    assert!(
        command_output.contains("after=less,unset"),
        "{command_output}"
    );
}
