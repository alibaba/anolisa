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
    assert_eq!(
        output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("please explain the last error")
                && event.component.as_deref() == Some("natural_language")
        }),
        bash_supports_command_not_found_handler()
    );
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
fn shell_host_bash_valid_cue_named_function_wins_over_natural_language() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-valid-cue-function-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(
        home_dir.join(".bashrc"),
        "Who() { printf '__who_function__:%s\\n' \"$*\"; }\n",
    )
    .expect("bashrc");
    let config = ShellHostConfig::new("valid-cue-function", &work_dir)
        .with_env("HOME", home_dir.display().to_string());

    let output = run_scripted_bash(&config, &[ScriptedInput::user_line("Who are you")])
        .expect("scripted bash");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(terminal.contains("__who_function__:are you"), "{terminal}");
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[test]
fn shell_host_bash_valid_cue_matrix_wins_over_natural_language() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-valid-cue-matrix-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    let bin_dir = work_dir.join("bin");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    std::fs::write(
        home_dir.join(".bashrc"),
        "alias Who='printf \"__alias_who__:%s\\\\n\"'\n",
    )
    .expect("bashrc");
    let kindly = bin_dir.join("Kindly");
    std::fs::write(
        &kindly,
        "#!/bin/sh\nprintf '__path_kindly__:%s\\n' \"$*\"\n",
    )
    .expect("Kindly executable");
    make_executable(&kindly);
    let han = bin_dir.join("帮我看看");
    std::fs::write(&han, "#!/bin/sh\nprintf '__han_path__:%s\\n' \"$*\"\n")
        .expect("Han executable");
    make_executable(&han);
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let config = with_raw_byte_readline(
        ShellHostConfig::new("valid-cue-matrix", &work_dir)
            .with_env("HOME", home_dir.display().to_string())
            .with_env("PATH", path),
    );

    let inputs = [
        "Who are you",
        "help file",
        "Kindly explain this",
        "帮我看看 当前目录",
    ];
    let output = run_scripted_bash(
        &config,
        &inputs
            .iter()
            .map(|input| ScriptedInput::user_line(*input))
            .collect::<Vec<_>>(),
    )
    .expect("scripted bash");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(terminal.contains("__alias_who__:are"), "{terminal}");
    assert!(
        terminal.contains("__path_kindly__:explain this"),
        "{terminal}"
    );
    assert!(terminal.contains("__han_path__:当前目录"), "{terminal}");
    for input in inputs {
        assert!(!output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input)
                && event.component.as_deref() == Some("natural_language")
        }));
    }
}

#[test]
fn shell_host_zsh_valid_cue_matrix_wins_over_natural_language() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-valid-cue-matrix-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    let bin_dir = work_dir.join("bin");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    std::fs::write(
        home_dir.join(".zshrc"),
        "alias Who='printf \"__zsh_alias_who__:%s\\\\n\"'\n\
         how() { printf '__zsh_function_how__:%s\\n' \"$*\"; }\n",
    )
    .expect("zshrc");
    let kindly = bin_dir.join("Kindly");
    std::fs::write(
        &kindly,
        "#!/bin/sh\nprintf '__zsh_path_kindly__:%s\\n' \"$*\"\n",
    )
    .expect("Kindly executable");
    make_executable(&kindly);
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let config = ShellHostConfig::new("zsh-valid-cue-matrix", &work_dir)
        .with_env("HOME", home_dir.display().to_string())
        .with_env("COSH_ZDOTDIR_ORIG", home_dir.display().to_string())
        .with_env("PATH", path);

    let inputs = [
        "Who are you",
        "how file",
        "Kindly explain this",
        "test this",
    ];
    let output = run_scripted_zsh(
        &config,
        &inputs
            .iter()
            .map(|input| ScriptedInput::user_line(*input))
            .collect::<Vec<_>>(),
    )
    .expect("scripted zsh");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(terminal.contains("__zsh_alias_who__:are"), "{terminal}");
    assert!(terminal.contains("__zsh_function_how__:file"), "{terminal}");
    assert!(
        terminal.contains("__zsh_path_kindly__:explain this"),
        "{terminal}"
    );
    for input in inputs {
        assert!(!output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input)
                && event.component.as_deref() == Some("natural_language")
        }));
    }
}

#[test]
fn shell_host_bash_missing_natural_language_closes_started_command() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }
    if !bash_supports_command_not_found_handler() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-missing-natural-language-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("missing-natural-language", &work_dir);
    config.native_mode = false;

    let output = run_scripted_bash(&config, &[ScriptedInput::user_line("Kindly explain this")])
        .expect("scripted bash");
    let intercept = output
        .events
        .iter()
        .find(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("Kindly explain this")
                && event.component.as_deref() == Some("natural_language")
        })
        .unwrap_or_else(|| panic!("natural-language intercept: {:?}", output.events));

    assert!(intercept.command_id.is_some(), "{:?}", output.events);
    assert!(
        intercept
            .routing
            .as_ref()
            .is_some_and(|routing| routing.top_level_missing && routing.proven),
        "{:?}",
        output.events
    );
    let ledger = build_command_blocks(&output.events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(!ledger
        .blocks
        .iter()
        .any(|block| block.command == "Kindly explain this"));
    assert!(
        !String::from_utf8_lossy(&output.terminal_output).contains("command not found"),
        "{}",
        String::from_utf8_lossy(&output.terminal_output)
    );
}

#[test]
fn shell_host_zsh_missing_natural_language_closes_started_command() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-missing-natural-language-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("zsh-missing-natural-language", &work_dir);
    config.native_mode = false;

    for input in ["Kindly explain this", "Just do it"] {
        let output =
            run_scripted_zsh(&config, &[ScriptedInput::user_line(input)]).expect("scripted zsh");
        let intercept = output
            .events
            .iter()
            .find(|event| {
                event.kind == ShellEventKind::UserInputIntercepted
                    && event.input.as_deref() == Some(input)
                    && event.component.as_deref() == Some("natural_language")
            })
            .unwrap_or_else(|| panic!("natural-language intercept: {:?}", output.events));

        assert!(intercept.command_id.is_some(), "{:?}", output.events);
        assert!(
            intercept
                .routing
                .as_ref()
                .is_some_and(|routing| routing.top_level_missing && routing.proven),
            "{:?}",
            output.events
        );
        let ledger = build_command_blocks(&output.events);
        assert!(!ledger.blocks.iter().any(|block| block.command == input));
        assert!(
            !String::from_utf8_lossy(&output.terminal_output).contains("command not found"),
            "{}",
            String::from_utf8_lossy(&output.terminal_output)
        );
    }
}

#[test]
fn shell_host_zsh_ambiguous_phrase_stays_in_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-ambiguous-phrase-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("zsh-ambiguous-phrase", &work_dir);
    config.native_mode = false;

    let output = run_scripted_zsh(
        &config,
        &[ScriptedInput::user_line(
            "_cosh_test_missing_ambiguous build",
        )],
    )
    .expect("scripted zsh");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(
        terminal.contains("command not found: _cosh_test_missing_ambiguous"),
        "{terminal}"
    );
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("_cosh_test_missing_ambiguous build")
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[test]
fn shell_host_bash_sensitive_missing_emits_raw_free_provenance() {
    if !bash_supports_command_not_found_handler() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-sensitive-missing-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let input = "missing_sensitive_cli --token=secretvalue";
    let output = run_scripted_bash(
        &ShellHostConfig::new("bash-sensitive-missing", &work_dir),
        &[ScriptedInput::user_line(input)],
    )
    .expect("scripted bash");
    let routing = output
        .events
        .iter()
        .find(|event| event.kind == ShellEventKind::CommandRoutingObserved)
        .unwrap_or_else(|| panic!("routing provenance: {:?}", output.events));

    assert_eq!(routing.component.as_deref(), Some("ambiguous"));
    assert!(routing.routing.as_ref().is_some_and(|metadata| {
        metadata.generation == 1
            && metadata.top_level_missing
            && metadata.proven
            && metadata.sensitive
            && !metadata.unsafe_input
    }));
    assert!(routing.input.is_none());
    assert!(routing.command.is_none());
    assert!(!format!("{:?}", output.events).contains("secretvalue"));
    assert!(String::from_utf8_lossy(&output.terminal_output).contains("command not found"));
}

#[test]
fn shell_host_missing_cksum_fails_closed_without_sensitive_provenance() {
    let input = "missing_sensitive_cli --token=secretvalue";
    let mut shells = vec!["bash"];
    if Command::new("zsh").arg("--version").output().is_ok() {
        shells.push("zsh");
    }
    let mut outputs = Vec::new();
    for shell in shells {
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-shell-{shell}-missing-cksum-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let bin_dir = work_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("stub bin dir");
        let cksum = bin_dir.join("cksum");
        std::fs::write(&cksum, "#!/bin/sh\nexit 1\n").expect("cksum stub");
        make_executable(&cksum);
        let path = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let config = ShellHostConfig::new(format!("{shell}-missing-cksum"), &work_dir)
            .with_env("PATH", path);
        let output = if shell == "bash" {
            run_scripted_bash(&config, &[ScriptedInput::user_line(input)])
        } else {
            run_scripted_zsh(&config, &[ScriptedInput::user_line(input)])
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        outputs.push((shell, output));
    }

    for (shell, output) in outputs {
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::CommandStarted
                && event.command.as_deref() == Some("<redacted sensitive command>")
        }));
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::CommandFailed && event.exit_code == Some(127)
        }));
        assert!(
            !output
                .events
                .iter()
                .any(|event| event.kind == ShellEventKind::CommandRoutingObserved),
            "{shell}: {:?}",
            output.events
        );
        assert!(
            !format!("{:?}", output.events).contains("secretvalue"),
            "{shell}: {:?}",
            output.events
        );
        assert!(
            String::from_utf8_lossy(&output.terminal_output).contains("command not found"),
            "{shell}: {}",
            String::from_utf8_lossy(&output.terminal_output)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn shell_host_linux_bash_natural_language_routes_directly_to_agent() {
    if !bash_supports_command_not_found_handler() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-who-are-you-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("bash-who-are-you", &work_dir);
    config.native_mode = false;

    for input in ["Who are you", "Just do it"] {
        let output =
            run_scripted_bash(&config, &[ScriptedInput::user_line(input)]).expect("scripted bash");
        let terminal = String::from_utf8_lossy(&output.terminal_output);

        assert!(!terminal.contains("command not found"), "{terminal}");
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input)
                && event.command_id.is_some()
                && event.component.as_deref() == Some("natural_language")
        }));
        let ledger = build_command_blocks(&output.events);
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        assert!(!ledger.blocks.iter().any(|block| block.command == input));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn shell_host_linux_bash_ambiguous_phrase_stays_in_shell() {
    if !bash_supports_command_not_found_handler() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-ambiguous-phrase-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("bash-ambiguous-phrase", &work_dir);
    config.native_mode = false;

    let output = run_scripted_bash(
        &config,
        &[ScriptedInput::user_line(
            "_cosh_test_missing_ambiguous build",
        )],
    )
    .expect("scripted bash");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(terminal.contains("command not found"), "{terminal}");
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("_cosh_test_missing_ambiguous build")
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn shell_host_linux_bash_ignores_inherited_system_missing_handler() {
    if !bash_supports_command_not_found_handler() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-system-missing-handler-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("bash-system-missing-handler", &work_dir).with_env(
        "BASH_FUNC_command_not_found_handle%%",
        "() { printf '__system_handler__\\n'; return 127; }",
    );
    config.native_mode = false;

    let output = run_scripted_bash(&config, &[ScriptedInput::user_line("Who are you")])
        .expect("scripted bash");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(!terminal.contains("__system_handler__"), "{terminal}");
    assert!(!terminal.contains("command not found"), "{terminal}");
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("Who are you")
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[test]
fn shell_host_zsh_ai_disabled_keeps_missing_natural_language_in_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-ai-disabled-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("zsh-ai-disabled", &work_dir).with_ai_enabled(false);
    config.native_mode = false;

    let output = run_scripted_zsh(&config, &[ScriptedInput::user_line("Kindly explain this")])
        .expect("scripted zsh");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(terminal.contains("command not found: Kindly"), "{terminal}");
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[test]
fn shell_host_zsh_nested_missing_is_not_treated_as_top_level_input() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-nested-missing-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(home_dir.join(".zshrc"), "ask() { please explain this; }\n").expect("zshrc");
    let config = ShellHostConfig::new("zsh-nested-missing", &work_dir)
        .with_env("HOME", home_dir.display().to_string())
        .with_env("COSH_ZDOTDIR_ORIG", home_dir.display().to_string());

    let output =
        run_scripted_zsh(&config, &[ScriptedInput::user_line("ask")]).expect("scripted zsh");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(terminal.contains("command not found: please"), "{terminal}");
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[test]
fn shell_host_bash_preserves_user_missing_handler_contract() {
    if Command::new("bash").arg("--version").output().is_err()
        || !bash_supports_command_not_found_handler()
    {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-user-missing-handler-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(
        home_dir.join(".bashrc"),
        "command_not_found_handle() {\n\
         printf '__user_handler__:%s:%s\\n' \"$#\" \"$*\"\n\
         handler_inner_missing\n\
         return 42\n\
         }\n",
    )
    .expect("bashrc");
    let config = ShellHostConfig::new("bash-user-missing-handler", &work_dir)
        .with_env("HOME", home_dir.display().to_string());

    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("terraform plan"),
            ScriptedInput::user_line("please explain this"),
        ],
    )
    .expect("scripted bash");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(
        terminal.contains("__user_handler__:2:terraform plan"),
        "{terminal}"
    );
    assert!(terminal.contains("handler_inner_missing"), "{terminal}");
    assert!(
        terminal.contains("__user_handler__:3:please explain this"),
        "{terminal}"
    );
    let ledger = ledger_from_output(&output);
    for command in ["terraform plan", "please explain this"] {
        let block = ledger
            .blocks
            .iter()
            .find(|block| block.command == command)
            .unwrap_or_else(|| panic!("{command} block"));
        assert_eq!(block.exit_code, 42, "{terminal}\n{:?}", output.events);
    }
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[test]
fn shell_host_zsh_preserves_user_missing_handler_contract() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-user-missing-handler-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(
        home_dir.join(".zshrc"),
        "command_not_found_handler() {\n\
         printf '__user_handler__:%s:%s\\n' \"$#\" \"$*\"\n\
         handler_inner_missing\n\
         return 42\n\
         }\n",
    )
    .expect("zshrc");
    let config = ShellHostConfig::new("zsh-user-missing-handler", &work_dir)
        .with_env("HOME", home_dir.display().to_string())
        .with_env("COSH_ZDOTDIR_ORIG", home_dir.display().to_string());

    let output = run_scripted_zsh(
        &config,
        &[
            ScriptedInput::user_line("terraform plan"),
            ScriptedInput::user_line("please explain this"),
        ],
    )
    .expect("scripted zsh");
    let terminal = String::from_utf8_lossy(&output.terminal_output);

    assert!(
        terminal.contains("__user_handler__:2:terraform plan"),
        "{terminal}"
    );
    assert!(
        terminal.contains("command not found: handler_inner_missing"),
        "{terminal}"
    );
    assert!(
        terminal.contains("__user_handler__:3:please explain this"),
        "{terminal}"
    );
    let ledger = ledger_from_output(&output);
    for command in ["terraform plan", "please explain this"] {
        let block = ledger
            .blocks
            .iter()
            .find(|block| block.command == command)
            .unwrap_or_else(|| panic!("{command} block"));
        assert_eq!(block.exit_code, 42, "{terminal}\n{:?}", output.events);
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::CommandRoutingObserved
                && event.command_id.as_deref() == Some(block.id.as_str())
                && event.routing.as_ref().is_some_and(|routing| routing.proven)
        }));
    }
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("natural_language")
    }));
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
fn shell_host_bash_tracks_native_history_file_changes() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-history-file-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    let relative_one = work_dir.join("relative-one");
    let relative_two = work_dir.join("relative-two");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::create_dir_all(&relative_one).expect("first relative dir");
    std::fs::create_dir_all(&relative_two).expect("second relative dir");

    let initial_history = home_dir.join("initial-history");
    let alternate_history = home_dir.join("alternate-history");
    let observed_history_files = work_dir.join("observed-history-files");
    std::fs::write(
        home_dir.join(".bashrc"),
        format!("export HISTFILE={}\n", shell_arg(&initial_history)),
    )
    .expect("bashrc");

    let install_marker_sink = format!(
        "_COSH_LAST_NATIVE_HISTORY_FILE=; \
         _cosh_emit_native_history_file_marker() {{ \
         printf '%s\\n' \"$1\" >> {}; \
         }}",
        shell_arg(&observed_history_files)
    );
    let config = ShellHostConfig::new("history-file-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line(install_marker_sink),
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

    let observed = std::fs::read_to_string(&observed_history_files)
        .expect("observed history files")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let expected = [
        initial_history,
        alternate_history,
        relative_one.join("relative-history"),
        relative_two.join("relative-history"),
    ]
    .into_iter()
    .map(|path| path.display().to_string())
    .collect::<Vec<_>>();
    assert_eq!(observed, expected);

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    let failed = ledger
        .blocks
        .iter()
        .find(|block| block.command == "false")
        .expect("false command block");
    assert_eq!(failed.exit_code, 1);
}

#[test]
fn shell_host_bash_tracks_history_file_changed_by_prompt_command() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-history-file-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");

    let initial_history = home_dir.join("initial-history");
    let prompt_history = home_dir.join("prompt-history");
    let observed_history_files = work_dir.join("observed-history-files");
    std::fs::write(
        home_dir.join(".bashrc"),
        format!(
            "export HISTFILE={}\n\
             export COSH_PROMPT_HISTORY_FILE={}\n\
             PROMPT_COMMAND='if [[ \"${{COSH_SWITCH_HISTORY:-}}\" == 1 ]]; then \
             HISTFILE=\"$COSH_PROMPT_HISTORY_FILE\"; unset COSH_SWITCH_HISTORY; fi'\n",
            shell_arg(&initial_history),
            shell_arg(&prompt_history)
        ),
    )
    .expect("bashrc");

    let install_marker_sink = format!(
        "_COSH_LAST_NATIVE_HISTORY_FILE=; \
         _cosh_emit_native_history_file_marker() {{ \
         printf '%s\\n' \"$1\" >> {}; \
         }}",
        shell_arg(&observed_history_files)
    );
    let config = ShellHostConfig::new("prompt-history-file-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line(install_marker_sink),
            ScriptedInput::user_line("export COSH_SWITCH_HISTORY=1"),
        ],
    )
    .expect("scripted bash pty");

    let observed = std::fs::read_to_string(&observed_history_files)
        .expect("observed history files")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            initial_history.display().to_string(),
            prompt_history.display().to_string(),
        ]
    );
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
    let observed_history_files = work_dir.join("observed-history-files");
    let install_marker_sink = format!(
        "_COSH_LAST_NATIVE_HISTORY_FILE=; \
         _cosh_emit_native_history_file_marker() {{ \
         printf '%s\\n' \"$1\" >> {}; \
         }}",
        shell_arg(&observed_history_files)
    );
    let mut config = ShellHostConfig::new("isolated-history-file-test", &work_dir);
    config.native_mode = false;

    run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line(install_marker_sink),
            ScriptedInput::user_line("export HISTFILE=/tmp/isolated-history"),
        ],
    )
    .expect("scripted isolated bash pty");

    assert!(!observed_history_files.exists());
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

#[test]
fn shell_host_bash_unexports_bashopts_while_keeping_extdebug_local() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }
    // BASHOPTS environment import exists since bash 4.1; on older hosts
    // (e.g. macOS /bin/bash 3.2) the leak vector cannot exist, so skip.
    let bashopts_supported = Command::new("bash")
        .env("BASHOPTS", "cdspell")
        .args(["--noprofile", "--norc", "-c", "shopt -q cdspell"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !bashopts_supported {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bashopts-unexport-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    // Child probe reporting whether extdebug leaked into a fresh bash. The
    // rc markers are expanded at runtime so the echoed input line can never
    // satisfy the assertions by itself.
    let probe_path = work_dir.join("bashopts-probe.sh");
    std::fs::write(
        &probe_path,
        "#!/bin/bash\nshopt -q extdebug; echo \"child-extdebug-rc=$?\"\n",
    )
    .expect("probe script");
    make_executable(&probe_path);

    // BASHOPTS arrives exported from the environment: bash keeps the export
    // attribute, which is exactly the leak precondition from issue #1782.
    let config =
        ShellHostConfig::new("bashopts-unexport-test", &work_dir).with_env("BASHOPTS", "cdspell");
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("shopt -q extdebug; echo \"host-extdebug-rc=$?\""),
            ScriptedInput::user_line("shopt -q cdspell; echo \"host-cdspell-rc=$?\""),
            ScriptedInput::user_line(
                "attrs=\"$(declare -p BASHOPTS)\"; attrs=\"${attrs%%BASHOPTS*}\"; \
                 [[ \"$attrs\" == *x* ]]; echo \"bashopts-export-rc=$?\"",
            ),
            ScriptedInput::user_line(format!("bash {}", shell_arg(&probe_path))),
        ],
    )
    .expect("scripted bash pty");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    // The marker keeps extdebug enabled in the interactive shell (DEBUG trap
    // return-1 suppression depends on it) and keeps imported options alive.
    assert!(terminal.contains("host-extdebug-rc=0"), "{terminal}");
    assert!(terminal.contains("host-cdspell-rc=0"), "{terminal}");
    // The export attribute must be gone so shopt changes stop propagating.
    assert!(terminal.contains("bashopts-export-rc=1"), "{terminal}");
    // A child bash spawned from the session must not start in extdebug mode
    // and must not trip the bashdb debugger-profile load.
    assert!(terminal.contains("child-extdebug-rc=1"), "{terminal}");
    assert!(!terminal.contains("bashdb"), "{terminal}");
}

#[test]
fn shell_host_bash_debug_trap_children_never_see_exported_extdebug() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }
    // Same BASHOPTS-import gate as the leak test above.
    let bashopts_supported = Command::new("bash")
        .env("BASHOPTS", "cdspell")
        .args(["--noprofile", "--norc", "-c", "shopt -q cdspell"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !bashopts_supported {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bashopts-trap-window-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    // A user rcfile runs before the marker's hook setup, so its DEBUG trap
    // is live while the marker enables extdebug. The trap records every
    // BASHOPTS frame and pipes child-bash stderr into evidence files; the
    // trailing ':' keeps the handler's exit status at 0 so it can never
    // suppress commands under extdebug. The leak detector is the exported
    // extdebug frame plus the child's bashdb load failure, because a leaking
    // child disables extdebug before shopt state could be probed.
    std::fs::write(
        home_dir.join(".bashrc"),
        "trap 'declare -p BASHOPTS >> \"$HOME/trap-log\" 2>/dev/null; bash -c \":\" 2>> \"$HOME/trap-err\"; :' DEBUG\n",
    )
    .expect("bashrc");

    let config = ShellHostConfig::new("bashopts-trap-window-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string())
        .with_env("BASHOPTS", "cdspell");
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("printf 'shell-alive-%s\\n' ok"),
            ScriptedInput::user_line(
                "leak=trap-child-extdebug-leaked; clean=window-clean; \
                 if grep -q \"^declare -[^ ]*x[^ ]* BASHOPTS=.*extdebug\" \"$HOME/trap-log\" || \
                    [[ -s \"$HOME/trap-err\" ]]; \
                 then echo \"__${leak}__\"; else echo \"trap-${clean}-ok\"; fi",
            ),
        ],
    )
    .expect("scripted bash pty");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    // The marker must drop the BASHOPTS export attribute before enabling
    // extdebug, so no DEBUG trap firing in between can leak it to children.
    // Markers only appear after runtime expansion, so the echoed input line
    // cannot satisfy either assertion by itself.
    assert!(
        !terminal.contains("__trap-child-extdebug-leaked__"),
        "{terminal}"
    );
    assert!(terminal.contains("trap-window-clean-ok"), "{terminal}");
    assert!(terminal.contains("shell-alive-ok"), "{terminal}");
}

#[test]
fn shell_host_bash_alias_expanded_commands_keep_preexec_markers() {
    // BASH_ALIASES (bash 4+) is required for the alias-aware guard; on
    // older bash (e.g. macOS /bin/bash 3.2) the guard degrades to pre-fix
    // behavior by design, so this test only runs on bash 4+.
    let version_probe = Command::new("bash")
        .args(["-c", "echo ${BASH_VERSINFO[0]}"])
        .output();
    let Ok(version_probe) = version_probe else {
        return;
    };
    let major = String::from_utf8_lossy(&version_probe.stdout)
        .trim()
        .parse::<u32>()
        .unwrap_or(0);
    if major < 4 {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-alias-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    let list_dir = work_dir.join("listing");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::create_dir_all(&list_dir).expect("list dir");
    let data_file = work_dir.join("data.txt");
    std::fs::write(&data_file, "needle\n").expect("data file");
    std::fs::write(
        home_dir.join(".bashrc"),
        "alias ls='ls --color=auto'\n\
         alias ll='ls -l'\n\
         alias lg='grep --color=auto -n'\n\
         alias wrap='env '\n",
    )
    .expect("bashrc");

    let config = ShellHostConfig::new("bash-alias-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let single = format!("ls {}", shell_arg(&list_dir));
    let chained = format!("ll {}", shell_arg(&list_dir));
    let assignment_prefixed = format!("FOO=1 ls {}", shell_arg(&list_dir));
    let compound = format!("ls {}; pwd", shell_arg(&list_dir));
    let pipeline = format!("ls {} | wc -l", shell_arg(&list_dir));
    let quoted_alias = format!("lg needle {}", shell_arg(&data_file));
    // Bash keeps alias-expanding the next word when an alias value ends
    // with a blank (alias wrap='env '), so `wrap ll <dir>` really runs
    // `env ls -l <dir>` and the guard must match that expansion.
    let trailing_blank = format!("wrap ll {}", shell_arg(&list_dir));
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line(single.clone()),
            ScriptedInput::user_line(chained.clone()),
            ScriptedInput::user_line(assignment_prefixed.clone()),
            ScriptedInput::user_line(compound.clone()),
            ScriptedInput::user_line(pipeline.clone()),
            ScriptedInput::user_line(quoted_alias.clone()),
            ScriptedInput::user_line(trailing_blank.clone()),
        ],
    )
    .expect("scripted bash pty");

    let ledger = ledger_from_output(&output);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    for expected in [
        &single,
        &chained,
        &assignment_prefixed,
        &compound,
        &pipeline,
        &quoted_alias,
        &trailing_blank,
    ] {
        let block = ledger
            .blocks
            .iter()
            .find(|block| block.command == **expected)
            .unwrap_or_else(|| {
                panic!(
                    "missing command block for {expected:?}; blocks: {:?}",
                    ledger
                        .blocks
                        .iter()
                        .map(|block| block.command.as_str())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(block.exit_code, 0, "{expected}");
    }
    // preexec must report the history original text, never the
    // alias-expanded variant.
    assert!(
        ledger
            .blocks
            .iter()
            .all(|block| !block.command.contains("--color=auto")),
        "{:?}",
        ledger
            .blocks
            .iter()
            .map(|block| block.command.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn shell_host_bash_alias_guard_survives_polluted_ifs() {
    // N6: the alias-aware guard must stay builtin-only and IFS-independent.
    let version_probe = Command::new("bash")
        .args(["-c", "echo ${BASH_VERSINFO[0]}"])
        .output();
    let Ok(version_probe) = version_probe else {
        return;
    };
    let major = String::from_utf8_lossy(&version_probe.stdout)
        .trim()
        .parse::<u32>()
        .unwrap_or(0);
    if major < 4 {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-ifs-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    let list_dir = work_dir.join("listing");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::create_dir_all(&list_dir).expect("list dir");
    std::fs::write(
        home_dir.join(".bashrc"),
        "alias ls='ls --color=auto'\nIFS=':'\n",
    )
    .expect("bashrc");

    let config = ShellHostConfig::new("bash-ifs-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let aliased = format!("ls {}", shell_arg(&list_dir));
    let output = run_scripted_bash(&config, &[ScriptedInput::user_line(aliased.clone())])
        .expect("scripted bash pty");

    let ledger = ledger_from_output(&output);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command == aliased)
        .unwrap_or_else(|| {
            panic!(
                "missing aliased block under polluted IFS; blocks: {:?}",
                ledger
                    .blocks
                    .iter()
                    .map(|block| block.command.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(block.exit_code, 0);
}

#[test]
fn shell_host_bash_stale_history_guard_still_intercepts_deduped_repeats() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-bash-histdedup-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(home_dir.join(".bashrc"), "HISTCONTROL=ignoredups\n").expect("bashrc");

    let config = ShellHostConfig::new("bash-histdedup-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("please explain the last error"),
            ScriptedInput::user_line("please explain the last error"),
        ],
    )
    .expect("scripted bash pty");

    let intercepts = output
        .events
        .iter()
        .filter(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("please explain the last error")
                && event.component.as_deref() == Some("natural_language")
        })
        .count();
    assert_eq!(intercepts, 2, "{:?}", output.events);
}
