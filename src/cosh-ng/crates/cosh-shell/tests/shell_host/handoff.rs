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
