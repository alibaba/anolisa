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

    // Post-#2138 the sensitive gate no longer short-circuits ahead of intent
    // classification: `--token=...` vetoes to intent=command, which keeps the
    // native error and emits raw-free provenance with the sensitive flag.
    assert_eq!(routing.component.as_deref(), Some("command"));
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

/// #2138: natural-language input carrying a secret routes to the agent like
/// regular NL (intercept event emitted, sensitive routing flag set) instead
/// of being silently vetoed to the native command-not-found error. The
/// harness returns journal-redacted events, so the intercept input must be
/// the whole-field redaction and the raw key must never appear (V3); the raw
/// text reaching the in-memory agent path is anchored in osc_tests.
#[test]
fn shell_host_sensitive_natural_language_routes_to_agent_with_flag() {
    let issue_input = "帮我安装下openclaw,模型使用qwen3.8-max,API Key: sk-fbaa6";
    let mut shells = Vec::new();
    if bash_supports_command_not_found_handler() {
        shells.push("bash");
    }
    if Command::new("zsh").arg("--version").output().is_ok() {
        shells.push("zsh");
    }
    for shell in shells {
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-shell-{shell}-sensitive-nl-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let config = ShellHostConfig::new(format!("{shell}-sensitive-nl"), &work_dir);
        let output = if shell == "bash" {
            run_scripted_bash(&config, &[ScriptedInput::user_line(issue_input)])
        } else {
            run_scripted_zsh(&config, &[ScriptedInput::user_line(issue_input)])
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));

        let terminal = String::from_utf8_lossy(&output.terminal_output);
        assert!(
            !terminal.contains("command not found") && !terminal.contains("未找到命令"),
            "{shell}: {terminal}"
        );
        let intercept = output
            .events
            .iter()
            .find(|event| {
                event.kind == ShellEventKind::UserInputIntercepted
                    && event.component.as_deref() == Some("natural_language")
            })
            .unwrap_or_else(|| panic!("{shell}: sensitive NL intercept: {:?}", output.events));
        assert_eq!(intercept.input.as_deref(), Some("<redacted>"), "{shell}");
        assert!(
            intercept
                .routing
                .as_ref()
                .is_some_and(|routing| routing.sensitive && routing.top_level_missing),
            "{shell}: {intercept:?}"
        );
        assert!(
            !format!("{:?}", output.events).contains("sk-fbaa6"),
            "{shell}: {:?}",
            output.events
        );
        let journal = std::fs::read_to_string(&output.journal_path).unwrap();
        assert!(!journal.contains("sk-fbaa6"), "{shell}: {journal}");
    }
}

#[test]
fn shell_host_missing_cksum_keeps_raw_free_sensitive_provenance() {
    // Post-#2138 the sensitive path shares the regular literal-first-word
    // identity check and no longer depends on the cksum fingerprint, so a
    // broken cksum only degrades the unsafe (invalid UTF-8) path. Sensitive
    // provenance must still be emitted, raw-free, with the native error.
    let input = "missing_sensitive_cli --token=secretvalue";
    let mut shells = Vec::new();
    if bash_supports_command_not_found_handler() {
        shells.push("bash");
    }
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
        let routing = output
            .events
            .iter()
            .find(|event| event.kind == ShellEventKind::CommandRoutingObserved)
            .unwrap_or_else(|| panic!("{shell}: routing provenance: {:?}", output.events));
        assert_eq!(routing.component.as_deref(), Some("command"), "{shell}");
        assert!(
            routing.routing.as_ref().is_some_and(|metadata| {
                metadata.top_level_missing && metadata.sensitive && !metadata.unsafe_input
            }),
            "{shell}: {routing:?}"
        );
        assert!(routing.input.is_none(), "{shell}");
        assert!(routing.command.is_none(), "{shell}");
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
fn routing_c4_zsh_stubs_intercept_bypass_only_commands() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-c4-zsh-stubs-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("routing-c4-zsh-stubs", &work_dir);
    let inputs = ["/draft", "/resume", "/session"];
    let steps = inputs
        .iter()
        .map(|input| ScriptedInput::user_line(*input))
        .collect::<Vec<_>>();
    let output = run_scripted_zsh(&config, &steps).expect("scripted zsh slash stubs");

    for input in inputs {
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input)
                && event.component.as_deref() == Some("slash")
        }));
        assert!(!output.events.iter().any(|event| {
            event.kind == ShellEventKind::CommandStarted && event.command.as_deref() == Some(input)
        }));
    }
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

// bash re-execs shebang-less scripts with --debugger only when built with
// debugger support, and the startup failure is only visible on hosts without
// the bashdb package. Probe the exact re-exec path so tests can skip
// anywhere the regression cannot manifest (e.g. macOS bash 3.2).
fn bash_reexecs_shebang_less_with_debugger(work_dir: &std::path::Path) -> bool {
    let probe_path = work_dir.join("shebang-less-probe");
    std::fs::write(&probe_path, "true\n").expect("probe script");
    make_executable(&probe_path);
    let probe = Command::new("bash")
        .args(["--noprofile", "--norc", "-c"])
        .arg(format!("shopt -s extdebug; {}", probe_path.display()))
        .output()
        .expect("debugger probe");
    let stderr = String::from_utf8_lossy(&probe.stderr);
    stderr.contains("bashdb") || stderr.contains("cannot start debugger")
}

#[test]
fn shell_host_bash_shebang_less_prompt_hook_avoids_debugger_reexec() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-hook-debugger-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    if !bash_reexecs_shebang_less_with_debugger(&work_dir) {
        return;
    }

    // Hook mirroring Alinux /etc/sysconfig/bash-prompt-history: inherited
    // PROMPT_COMMAND points at a shebang-less executable script, so the
    // marker's eval can only run it through the ENOEXEC re-exec fallback.
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    let hook_log = work_dir.join("hook-ran.log");
    let hook_path = work_dir.join("bash-prompt-history");
    std::fs::write(
        &hook_path,
        format!("echo prompt-hook-ran >> {}\n", hook_log.display()),
    )
    .expect("hook script");
    make_executable(&hook_path);

    let config = ShellHostConfig::new("prompt-hook-debugger-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string())
        .with_env("PROMPT_COMMAND", hook_path.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("echo ok"),
            ScriptedInput::user_line("shopt -q extdebug; echo \"post-hook-extdebug-rc=$?\""),
        ],
    )
    .expect("scripted bash pty");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    // The hook must still run — the fix silences the debugger re-exec,
    // not the user prompt command.
    let hook_ran = std::fs::read_to_string(&hook_log).unwrap_or_default();
    assert!(hook_ran.contains("prompt-hook-ran"), "{terminal}");
    assert!(!terminal.contains("bashdb"), "{terminal}");
    assert!(!terminal.contains("cannot start debugger"), "{terminal}");
    // extdebug must be back on for the next real command: the DEBUG trap
    // return-1 suppression depends on it.
    assert!(terminal.contains("post-hook-extdebug-rc=0"), "{terminal}");
}

#[test]
fn shell_host_bash_shebang_less_prompt_hook_array_form_avoids_debugger_reexec() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }
    // Array PROMPT_COMMAND only exists since bash 5.1.
    let version_probe = Command::new("bash")
        .args(["-c", "echo ${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"])
        .output();
    let Ok(version_probe) = version_probe else {
        return;
    };
    let version = String::from_utf8_lossy(&version_probe.stdout);
    let mut parts = version
        .split_whitespace()
        .filter_map(|part| part.parse::<u32>().ok());
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if (major, minor) < (5, 1) {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-hook-array-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    if !bash_reexecs_shebang_less_with_debugger(&work_dir) {
        return;
    }

    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    let hook_log = work_dir.join("hook-ran.log");
    let hook_path = work_dir.join("bash-prompt-history");
    std::fs::write(
        &hook_path,
        format!("echo array-hook-ran >> {}\n", hook_log.display()),
    )
    .expect("hook script");
    make_executable(&hook_path);
    // Array-form PROMPT_COMMAND captured from the user rcfile. The leading
    // `return 0` element pins the eval helper frame: an early-returning hook
    // must not skip the remaining elements or the extdebug restore.
    std::fs::write(
        home_dir.join(".bashrc"),
        format!("PROMPT_COMMAND=('return 0' '{}')\n", hook_path.display()),
    )
    .expect("bashrc");

    let config = ShellHostConfig::new("prompt-hook-array-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line("echo ok"),
            ScriptedInput::user_line("shopt -q extdebug; echo \"post-hook-extdebug-rc=$?\""),
        ],
    )
    .expect("scripted bash pty");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    let hook_ran = std::fs::read_to_string(&hook_log).unwrap_or_default();
    assert!(hook_ran.contains("array-hook-ran"), "{terminal}");
    assert!(!terminal.contains("bashdb"), "{terminal}");
    assert!(!terminal.contains("cannot start debugger"), "{terminal}");
    assert!(terminal.contains("post-hook-extdebug-rc=0"), "{terminal}");
}

fn bash_extdebug_clears_trace_options() -> bool {
    // `shopt -u extdebug` also clears errtrace/functrace on every bash that
    // implements the implication (4.4 through 5.x); skip only where bash is
    // too old to link the flags at all.
    Command::new("bash")
        .args([
            "--noprofile",
            "--norc",
            "-c",
            "set -E; set -T; shopt -s extdebug; shopt -u extdebug; \
             [[ ! -o errtrace && ! -o functrace ]]",
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[test]
fn shell_host_bash_prompt_hook_preserves_trace_option_inheritance() {
    if !bash_extdebug_clears_trace_options() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-hook-trace-options-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    let trace_log = work_dir.join("trace-options.log");
    std::fs::write(
        home_dir.join(".bashrc"),
        format!(
            "trap 'printf \"ERR\\t%s\\t%s\\n\" \"$BASH_COMMAND\" \
             \"${{FUNCNAME[*]}}\" >> {trace_log}' ERR\n\
             trap 'printf \"DEBUG\\t%s\\t%s\\n\" \"$BASH_COMMAND\" \
             \"${{FUNCNAME[*]}}\" >> {trace_log}' DEBUG\n\
             _user_hook() {{\n\
               false\n\
             }}\n\
             PROMPT_COMMAND=_user_hook\n",
            trace_log = trace_log.display()
        ),
    )
    .expect("bashrc");

    let config = ShellHostConfig::new("prompt-hook-trace-options-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[ScriptedInput::user_line("echo trace-options-preserved")],
    )
    .expect("scripted bash pty");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    let trace_log = std::fs::read_to_string(&trace_log).unwrap_or_default();
    let has_hook_trace = |event: &str| {
        let prefix = format!("{event}\tfalse\t");
        trace_log.lines().any(|line| {
            let Some(functions) = line.strip_prefix(&prefix) else {
                return false;
            };
            functions
                .split_whitespace()
                .any(|function| function == "_user_hook")
        })
    };
    assert!(
        has_hook_trace("ERR"),
        "ERR trap did not reach _user_hook false command\n{terminal}\n{trace_log}"
    );
    assert!(
        has_hook_trace("DEBUG"),
        "DEBUG trap did not reach _user_hook false command\n{terminal}\n{trace_log}"
    );
    assert!(terminal.contains("trace-options-preserved"), "{terminal}");
}

#[test]
fn shell_host_bash_prompt_hook_survives_debug_trap_and_self_heals() {
    if !bash_extdebug_clears_trace_options() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-hook-debug-trap-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    // A hook that installs a DEBUG trap ending in `return 0` unwinds every
    // function frame past the in-function extdebug restore. The session must
    // stay usable and extdebug must self-heal after the user clears the trap.
    std::fs::write(
        home_dir.join(".bashrc"),
        "_user_hook() {\n\
           if [[ -z \"${_poisoned:-}\" ]]; then\n\
             _poisoned=1\n\
             trap 'return 0' DEBUG\n\
           fi\n\
         }\n\
         PROMPT_COMMAND=_user_hook\n",
    )
    .expect("bashrc");

    let config = ShellHostConfig::new("prompt-hook-debug-trap-test", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    let output = run_scripted_bash(
        &config,
        &[
            // Executes natively while the poisoned trap is live: with
            // extdebug off the top-level `return` only prints an error and
            // the command still runs — the session is not bricked.
            ScriptedInput::user_line("trap - DEBUG; echo session-usable"),
            // The prompt cycle after the trap removal ran the in-function
            // restore again, so extdebug must be back on.
            ScriptedInput::user_line("shopt -q extdebug; echo \"post-heal-extdebug-rc=$?\""),
        ],
    )
    .expect("scripted bash pty");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(terminal.contains("session-usable"), "{terminal}");
    assert!(terminal.contains("post-heal-extdebug-rc=0"), "{terminal}");
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
    if !bash_supports_command_not_found_handler() {
        eprintln!("SKIP: bash command_not_found_handle capability is unavailable");
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

// Issue #1919: a natural-language prompt whose IFS first token contains a
// slash never reaches command_not_found_handle (bash executes the token as
// a path), so the DEBUG trap reclassifies it with the missing-path context
// and intercepts before execution.
#[test]
fn shell_host_bash_missing_path_natural_language_intercepts() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-missing-path-nl-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    // Pin a UTF-8 locale: a real terminal typing Chinese is UTF-8, while CI
    // runners default to C, where readline mangles multi-byte input bytes
    // before they ever reach the DEBUG trap (same precedent as
    // governance.rs raw-hook-test).
    let mut config = ShellHostConfig::new("missing-path-nl", &work_dir);
    config
        .env_overrides
        .push(("LANG".to_string(), "C.UTF-8".to_string()));
    config
        .env_overrides
        .push(("LC_ALL".to_string(), "C.UTF-8".to_string()));

    let prompt = "你读一下，并安装这个skill：/nonexistent-cosh-1919-probe/SKILL.md";
    let output =
        run_scripted_bash(&config, &[ScriptedInput::user_line(prompt)]).expect("scripted bash pty");

    let intercept = output
        .events
        .iter()
        .find(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(prompt)
                && event.component.as_deref() == Some("natural_language")
        })
        .unwrap_or_else(|| {
            panic!(
                "missing-path natural-language intercept: {:?}",
                output.events
            )
        });
    // Pre-execution intercepts are shaped like slash/agent-marker intercepts
    // (no top_level_missing correlation: the command never started, so there
    // is no in-flight attempt to correlate with).
    assert!(intercept.routing.is_none(), "{:?}", output.events);
    // Interception must prevent execution: no command block and no native
    // bash path error may appear for the prompt (I4).
    let ledger = build_command_blocks(&output.events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(!ledger.blocks.iter().any(|block| block.command == prompt));
    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(
        !terminal.contains("No such file or directory"),
        "{terminal}"
    );
}

/// #2138 review round 2: the missing-path route (#1919) must not keep its
/// own secret veto — a slash-bearing NL prompt carrying a key intercepts
/// like the CNF route, with the sensitive routing flag and the journal
/// whole-field redaction (raw key never reaches durable evidence).
#[test]
fn shell_host_bash_sensitive_missing_path_natural_language_intercepts() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-missing-path-sensitive-nl-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let mut config = ShellHostConfig::new("missing-path-sensitive-nl", &work_dir);
    config
        .env_overrides
        .push(("LANG".to_string(), "C.UTF-8".to_string()));
    config
        .env_overrides
        .push(("LC_ALL".to_string(), "C.UTF-8".to_string()));

    let prompt =
        "你读一下，并安装这个skill：/nonexistent-cosh-1919-probe/SKILL.md API Key: sk-fbaa6";
    let output =
        run_scripted_bash(&config, &[ScriptedInput::user_line(prompt)]).expect("scripted bash pty");

    let intercept = output
        .events
        .iter()
        .find(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.component.as_deref() == Some("natural_language")
        })
        .unwrap_or_else(|| {
            panic!(
                "sensitive missing-path natural-language intercept: {:?}",
                output.events
            )
        });
    // The harness returns journal-redacted events: the sensitive flag must
    // trigger the whole-field redaction and no correlation exists (the
    // command never started, so top_level_missing stays false).
    assert_eq!(intercept.input.as_deref(), Some("<redacted>"));
    assert!(
        intercept
            .routing
            .as_ref()
            .is_some_and(|routing| routing.sensitive && !routing.top_level_missing),
        "{intercept:?}"
    );
    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(
        !terminal.contains("No such file or directory"),
        "{terminal}"
    );
    assert!(
        !format!("{:?}", output.events).contains("sk-fbaa6"),
        "{:?}",
        output.events
    );
    let journal = std::fs::read_to_string(&output.journal_path).unwrap();
    assert!(!journal.contains("sk-fbaa6"), "{journal}");
}

#[test]
fn routing_c1_cnf_han_tier_a_routes_to_agent() {
    for shell in ["bash", "zsh"] {
        if shell == "bash" && !bash_supports_command_not_found_handler() {
            continue;
        }
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c1-cnf-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut config = ShellHostConfig::new(format!("routing-c1-cnf-{shell}"), &work_dir);
        config.native_mode = false;
        let mut inputs = vec![
            "使用 git log --since=\"1 day ago\" --format=\"%h %s (%an, %ar)\" 总结",
            "你还好吗？ 我想问问",
        ];
        if shell == "bash" {
            inputs.push("你还好吗? 我想问问");
        }
        let steps = inputs
            .iter()
            .map(|input| ScriptedInput::user_line(*input))
            .collect::<Vec<_>>();
        let output = if shell == "bash" {
            run_scripted_bash(&config, &steps)
        } else {
            run_scripted_zsh(&config, &steps)
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        for input in inputs {
            assert!(
                output.events.iter().any(|event| {
                    event.kind == ShellEventKind::UserInputIntercepted
                        && event.input.as_deref() == Some(input)
                        && event.component.as_deref() == Some("natural_language")
                }),
                "{shell}: {input:?}: {:?}\n{}",
                output.events,
                String::from_utf8_lossy(&output.terminal_output)
            );
        }
    }
}

#[test]
fn isolated_bash_ignores_inherited_prompt_and_history_filters() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }
    let supports_command_not_found = bash_supports_command_not_found_handler();
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-isolated-prompt-command-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let inherited_inputrc = work_dir.join("inherited-inputrc");
    std::fs::write(
        &inherited_inputrc,
        "set input-meta off\nset convert-meta on\nset output-meta off\n",
    )
    .expect("inherited inputrc");
    let inherited_marker = work_dir.join("inherited-prompt-command-ran");
    let mut config = ShellHostConfig::new("isolated-prompt-command", &work_dir)
        .with_env(
            "PROMPT_COMMAND",
            format!("printf inherited > {}", shell_arg(&inherited_marker)),
        )
        .with_env("HISTIGNORE", "*")
        .with_env("HISTSIZE", "0")
        .with_env("HISTFILESIZE", "0")
        .with_env("INPUTRC", inherited_inputrc.display().to_string());
    config.native_mode = false;
    let input = "解释 inherited history filter";

    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line(
                "printf '__cosh_hist_limits__%s:%s\\n' \"$HISTSIZE\" \"$HISTFILESIZE\"",
            ),
            ScriptedInput::user_line(input),
        ],
    )
    .expect("isolated scripted bash");

    assert!(!inherited_marker.exists());
    assert!(
        String::from_utf8_lossy(&output.terminal_output).contains("__cosh_hist_limits__1000:1000")
    );
    if supports_command_not_found {
        assert!(output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input)
                && event.component.as_deref() == Some("natural_language")
        }));
    }
}

#[test]
fn isolated_zsh_preserves_inputrc_override() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-isolated-zsh-inputrc-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let inputrc = work_dir.join("custom-inputrc");
    let mut config = ShellHostConfig::new("isolated-zsh-inputrc", &work_dir)
        .with_env("INPUTRC", inputrc.display().to_string());
    config.native_mode = false;

    let output = run_scripted_zsh(
        &config,
        &[ScriptedInput::user_line(
            "printf '__cosh_inputrc__%s\\n' \"$INPUTRC\"",
        )],
    )
    .expect("isolated scripted zsh");

    assert!(String::from_utf8_lossy(&output.terminal_output)
        .contains(&format!("__cosh_inputrc__{}", inputrc.display())));
}

#[test]
fn routing_c1_zsh_ascii_question_unmatched_routes_to_agent() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let input = "你还好吗? 我想问问";
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-routing-c1-zsh-question-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("routing-c1-zsh-question", &work_dir);
    let output =
        run_scripted_zsh(&config, &[ScriptedInput::user_line(input)]).expect("scripted zsh");
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some(input)
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn routing_c1_missing_path_han_tier_a_routes_to_agent() {
    if !bash_supports_command_not_found_handler() {
        return;
    }
    let input = "打开./不存在 --dry-run \"x (preview)\"";
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-routing-c1-missing-path-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut config = ShellHostConfig::new("routing-c1-missing-path", &work_dir);
    config.native_mode = false;
    let output =
        run_scripted_bash(&config, &[ScriptedInput::user_line(input)]).expect("scripted bash");
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some(input)
            && event.component.as_deref() == Some("natural_language")
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn routing_c1_stale_history_repeated_han_prompt_routes_twice() {
    if !bash_supports_command_not_found_handler() {
        return;
    }
    let input = "解释 git log --format=\"%h (%an)\"";
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-routing-c1-stale-history-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let home_dir = work_dir.join("home");
    std::fs::create_dir_all(&home_dir).expect("home dir");
    std::fs::write(home_dir.join(".bashrc"), "HISTCONTROL=ignoredups\n").expect("bashrc");
    let mut config = ShellHostConfig::new("routing-c1-stale-history", &work_dir)
        .with_env("HOME", home_dir.display().to_string());
    config.native_mode = false;
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line(input),
            ScriptedInput::user_line(input),
        ],
    )
    .expect("scripted bash");
    let intercepts = output
        .events
        .iter()
        .filter(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input)
                && event.component.as_deref() == Some("natural_language")
        })
        .count();
    assert_eq!(intercepts, 2, "{:?}", output.events);
}

#[test]
fn routing_c1_tier_b_side_effect_stays_native() {
    for shell in ["bash", "zsh"] {
        if shell == "bash" && !bash_supports_command_not_found_handler() {
            continue;
        }
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c1-tier-b-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&work_dir).expect("work dir");
        let side = work_dir.join("side-effect");
        let input = format!("解释 \"$(touch {})\"", shell_arg(&side));
        let config = ShellHostConfig::new(format!("routing-c1-tier-b-{shell}"), &work_dir);
        let output = if shell == "bash" {
            run_scripted_bash(&config, &[ScriptedInput::user_line(input.clone())])
        } else {
            run_scripted_zsh(&config, &[ScriptedInput::user_line(input.clone())])
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        assert!(side.exists(), "{shell}: command substitution did not run");
        assert!(!output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input.as_str())
        }));
    }
}

#[test]
fn routing_c1_zsh_glob_qualifier_stays_native() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-routing-c1-zsh-glob-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    std::fs::write(work_dir.join("entry"), "x\n").expect("glob entry");
    let side = work_dir.join("glob-side-effect");
    let input = format!("解释 *(e:'touch {}':)", side.display());
    let config = ShellHostConfig::new("routing-c1-zsh-glob", &work_dir);
    let output = run_scripted_zsh(&config, &[ScriptedInput::user_line(input.clone())])
        .expect("scripted zsh");
    assert!(side.exists(), "glob qualifier did not run");
    assert!(!output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some(input.as_str())
    }));
}

#[test]
fn routing_c1_valid_han_command_stays_native() {
    for shell in ["bash", "zsh"] {
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c1-valid-han-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let bin_dir = work_dir.join("bin");
        let home_dir = work_dir.join("home");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");
        std::fs::create_dir_all(&home_dir).expect("home dir");
        let executable = bin_dir.join("解释");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '__han_exec__:%s\\n' \"$1\"\n",
        )
        .expect("han executable");
        make_executable(&executable);
        let path = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let rc = "解释函数() { printf '__han_function__:%s\\n' \"$1\"; }\n\
                  alias 解释别名=\"printf '__han_alias__:%s\\n'\"\n";
        std::fs::write(
            home_dir.join(if shell == "bash" { ".bashrc" } else { ".zshrc" }),
            rc,
        )
        .expect("shell rc");
        let mut config = ShellHostConfig::new(format!("routing-c1-valid-han-{shell}"), &work_dir)
            .with_env("PATH", path)
            .with_env("HOME", home_dir.display().to_string());
        if shell == "zsh" {
            config = config.with_env("COSH_ZDOTDIR_ORIG", home_dir.display().to_string());
        }
        let inputs = ["解释 ok", "解释函数 ok", "解释别名 ok"];
        let steps = inputs
            .iter()
            .map(|input| ScriptedInput::user_line(*input))
            .collect::<Vec<_>>();
        let output = if shell == "bash" {
            run_scripted_bash(&config, &steps)
        } else {
            run_scripted_zsh(&config, &steps)
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        let terminal = String::from_utf8_lossy(&output.terminal_output);
        for marker in ["__han_exec__:ok", "__han_function__:ok", "__han_alias__:ok"] {
            assert!(terminal.contains(marker), "{shell}: {marker}: {terminal}");
        }
        for input in inputs {
            assert!(!output.events.iter().any(|event| {
                event.kind == ShellEventKind::UserInputIntercepted
                    && event.input.as_deref() == Some(input)
            }));
        }
    }
}

fn assert_routing_c2_quote_cnf(shell: &str) {
    if shell == "bash" && !bash_supports_command_not_found_handler() {
        return;
    }
    if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let inputs = ["'子曰 三人行'后续 请解释", "\"子 曰\"后续 请解释"];
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-routing-c2-quote-{shell}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new(format!("routing-c2-quote-{shell}"), &work_dir);
    let steps = inputs
        .iter()
        .map(|input| ScriptedInput::user_line(*input))
        .collect::<Vec<_>>();
    let output = if shell == "bash" {
        run_scripted_bash(&config, &steps)
    } else {
        run_scripted_zsh(&config, &steps)
    }
    .unwrap_or_else(|error| panic!("{shell}: {error}"));
    for input in inputs {
        assert!(
            output.events.iter().any(|event| {
                event.kind == ShellEventKind::UserInputIntercepted
                    && event.input.as_deref() == Some(input)
                    && event.component.as_deref() == Some("natural_language")
            }),
            "{shell}: {input:?}: {:?}\n{}",
            output.events,
            String::from_utf8_lossy(&output.terminal_output)
        );
    }
}

#[test]
fn routing_c2_bash_quote_cnf_routes_to_agent() {
    assert_routing_c2_quote_cnf("bash");
}

#[test]
fn routing_c2_zsh_quote_cnf_routes_to_agent() {
    assert_routing_c2_quote_cnf("zsh");
}

#[test]
fn routing_c2_expansion_drift_and_matched_arguments_stay_native() {
    for shell in ["bash", "zsh"] {
        if shell == "bash" && !bash_supports_command_not_found_handler() {
            continue;
        }
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c2-drift-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let home_dir = work_dir.join("home");
        std::fs::create_dir_all(&home_dir).expect("home dir");
        std::fs::write(work_dir.join("match.log"), "x\n").expect("glob match");
        let side = work_dir.join("alias-side");
        let rc = format!("alias 解释='解释; touch {}'\n", side.display());
        std::fs::write(
            home_dir.join(if shell == "bash" { ".bashrc" } else { ".zshrc" }),
            rc,
        )
        .expect("shell rc");
        let mut config = ShellHostConfig::new(format!("routing-c2-drift-{shell}"), &work_dir)
            .with_env("HOME", home_dir.display().to_string());
        if shell == "zsh" {
            config = config.with_env("COSH_ZDOTDIR_ORIG", home_dir.display().to_string());
        }
        let alias_input = "解释";
        let glob_input = "说明 *.log";
        let cd_input = format!("cd {}", shell_arg(&work_dir));
        let output = if shell == "bash" {
            run_scripted_bash(
                &config,
                &[
                    ScriptedInput::user_line(cd_input.clone()),
                    ScriptedInput::user_line(alias_input),
                    ScriptedInput::user_line(glob_input),
                ],
            )
        } else {
            run_scripted_zsh(
                &config,
                &[
                    ScriptedInput::user_line(cd_input),
                    ScriptedInput::user_line(alias_input),
                    ScriptedInput::user_line(glob_input),
                ],
            )
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        assert!(side.exists(), "{shell}: alias side effect missing");
        for input in [alias_input, glob_input] {
            assert!(
                !output.events.iter().any(|event| {
                    event.kind == ShellEventKind::UserInputIntercepted
                        && event.input.as_deref() == Some(input)
                }),
                "{shell}: {input}: {:?}",
                output.events
            );
        }
    }
}

#[test]
fn routing_c2_long_mixed_language_prompts_route_to_agent() {
    // Issue #2053: zsh preexec abbreviates its second argument for long
    // inputs; comparing the canonicalized command against it produced a
    // false expansion-drift signal that pushed long CJK/ASCII prompts to
    // the native command-not-found path instead of the Agent. Cover both
    // sides of the abbreviated-text boundary plus a Chinese/English space
    // variation, and keep bash as the consistency control.
    let inputs = [
        // Above the abbreviation boundary (the original user report).
        "刚才没有恢复，不过 macOS 显示管理器认为屏幕在线",
        // Space removed at the Chinese/English boundary.
        "刚才没有恢复，不过macOS 显示管理器认为屏幕在线",
        // Below the abbreviation boundary.
        "刚才没有恢复 请解释",
    ];
    for shell in ["bash", "zsh"] {
        if shell == "bash" && !bash_supports_command_not_found_handler() {
            continue;
        }
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c2-long-nl-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let config = ShellHostConfig::new(format!("routing-c2-long-nl-{shell}"), &work_dir);
        let steps = inputs
            .iter()
            .map(|input| ScriptedInput::user_line(*input))
            .collect::<Vec<_>>();
        let output = if shell == "bash" {
            run_scripted_bash(&config, &steps)
        } else {
            run_scripted_zsh(&config, &steps)
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        let terminal = String::from_utf8_lossy(&output.terminal_output);
        for input in inputs {
            let first_word = input.split_whitespace().next().expect("first word");
            assert!(
                !terminal.contains(&format!("command not found: {first_word}")),
                "{shell}: {input:?} hit native command-not-found: {terminal}"
            );
            assert!(
                output.events.iter().any(|event| {
                    event.kind == ShellEventKind::UserInputIntercepted
                        && event.input.as_deref() == Some(input)
                        && event.component.as_deref() == Some("natural_language")
                }),
                "{shell}: {input:?}: {:?}\n{terminal}",
                output.events
            );
            assert!(
                !output.events.iter().any(|event| {
                    event.kind == ShellEventKind::CommandFailed
                        && event.command.as_deref() == Some(input)
                        && event.exit_code == Some(127)
                }),
                "{shell}: {input:?} failed with exit 127: {:?}\n{terminal}",
                output.events
            );
        }
    }
}

#[test]
fn routing_c2_nested_provenance_marks_only_the_outer_missing_command() {
    let input = "解释 \"$(解释)\"";
    for shell in ["bash", "zsh"] {
        if shell == "bash" && !bash_supports_command_not_found_handler() {
            continue;
        }
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c2-nested-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let config = ShellHostConfig::new(format!("routing-c2-nested-{shell}"), &work_dir);
        let output = if shell == "bash" {
            run_scripted_bash(&config, &[ScriptedInput::user_line(input)])
        } else {
            run_scripted_zsh(&config, &[ScriptedInput::user_line(input)])
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        let proven = output
            .events
            .iter()
            .filter(|event| {
                event
                    .routing
                    .as_ref()
                    .is_some_and(|routing| routing.top_level_missing && routing.proven)
            })
            .count();
        assert_eq!(proven, 1, "{shell}: {:?}", output.events);
        assert!(!output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(input)
        }));
    }
}

#[test]
fn routing_c2_nested_provenance_rejects_same_token_from_shell_function() {
    for shell in ["bash", "zsh"] {
        if shell == "bash" && !bash_supports_command_not_found_handler() {
            continue;
        }
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c2-function-nested-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let home_dir = work_dir.join("home");
        std::fs::create_dir_all(&home_dir).expect("home dir");
        let rc = if shell == "bash" {
            "解释() { unset -f 解释; 解释; }\n"
        } else {
            "解释() { unfunction 解释; 解释; }\n"
        };
        std::fs::write(
            home_dir.join(if shell == "bash" { ".bashrc" } else { ".zshrc" }),
            rc,
        )
        .expect("shell rc");
        let mut config =
            ShellHostConfig::new(format!("routing-c2-function-nested-{shell}"), &work_dir)
                .with_env("HOME", home_dir.display().to_string());
        if shell == "zsh" {
            config = config.with_env("COSH_ZDOTDIR_ORIG", home_dir.display().to_string());
        }
        let output = if shell == "bash" {
            run_scripted_bash(&config, &[ScriptedInput::user_line("解释")])
        } else {
            run_scripted_zsh(&config, &[ScriptedInput::user_line("解释")])
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        assert!(
            !output.events.iter().any(|event| {
                event.kind == ShellEventKind::UserInputIntercepted
                    || event
                        .routing
                        .as_ref()
                        .is_some_and(|routing| routing.top_level_missing && routing.proven)
            }),
            "{shell}: {:?}",
            output.events
        );
    }
}

#[test]
fn routing_c2_delegate_unsupported_escape_and_matched_glob() {
    for shell in ["bash", "zsh"] {
        if shell == "bash" && !bash_supports_command_not_found_handler() {
            continue;
        }
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c2-unsupported-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&work_dir).expect("work dir");
        std::fs::write(work_dir.join("中文a"), "x\n").expect("glob match");
        let inputs = [r"解释\ 一下", "中文?"];
        let config = ShellHostConfig::new(format!("routing-c2-unsupported-{shell}"), &work_dir);
        let mut steps = vec![ScriptedInput::user_line(format!(
            "cd {}",
            shell_arg(&work_dir)
        ))];
        steps.extend(inputs.iter().map(|input| ScriptedInput::user_line(*input)));
        let output = if shell == "bash" {
            run_scripted_bash(&config, &steps)
        } else {
            run_scripted_zsh(&config, &steps)
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        for input in inputs {
            assert!(
                !output.events.iter().any(|event| {
                    event.kind == ShellEventKind::UserInputIntercepted
                        && event.input.as_deref() == Some(input)
                }),
                "{shell}: {input}: {:?}",
                output.events
            );
        }
    }
}

#[test]
fn routing_c2_valid_quoted_command_and_inner_whitespace_keep_their_owners() {
    for shell in ["bash", "zsh"] {
        if shell == "zsh" && Command::new("zsh").arg("--version").output().is_err() {
            continue;
        }
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-routing-c2-valid-quoted-{shell}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let bin_dir = work_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");
        let executable = bin_dir.join("中文 命令");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '__quoted_exec__:%s\\n' \"$1\"\n",
        )
        .expect("quoted executable");
        make_executable(&executable);
        let path = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let config = ShellHostConfig::new(format!("routing-c2-valid-quoted-{shell}"), &work_dir)
            .with_env("PATH", path);
        let native = "\"中文 命令\" ok";
        let natural = "解释  一下";
        let output = if shell == "bash" {
            run_scripted_bash(
                &config,
                &[
                    ScriptedInput::user_line(native),
                    ScriptedInput::user_line(natural),
                ],
            )
        } else {
            run_scripted_zsh(
                &config,
                &[
                    ScriptedInput::user_line(native),
                    ScriptedInput::user_line(natural),
                ],
            )
        }
        .unwrap_or_else(|error| panic!("{shell}: {error}"));
        assert!(
            String::from_utf8_lossy(&output.terminal_output).contains("__quoted_exec__:ok"),
            "{shell}: {}",
            String::from_utf8_lossy(&output.terminal_output)
        );
        assert!(!output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some(native)
        }));
        if shell == "zsh" || bash_supports_command_not_found_handler() {
            assert!(
                output.events.iter().any(|event| {
                    event.kind == ShellEventKind::UserInputIntercepted
                        && event.input.as_deref() == Some(natural)
                }),
                "{shell}: {:?}",
                output.events
            );
        }
    }
}

// Issue #1919 fail-closed counterproofs: the missing-path branch must never
// fire for existing paths (I1/D6) or plain-English typo paths (I2/D3) —
// bash native behavior stays byte-identical. (The former I3 secret
// counterproof is retired by #2138: secret-bearing missing-path NL now
// intercepts with the sensitive flag, anchored in
// shell_host_bash_sensitive_missing_path_natural_language_intercepts.)
#[test]
fn shell_host_bash_missing_path_counterproofs_stay_native() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-missing-path-native-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    // I1: existing executable script keeps running natively.
    let probe_path = work_dir.join("probe-1919.sh");
    let executed_path = work_dir.join("probe-1919-executed");
    std::fs::write(
        &probe_path,
        format!("#!/bin/sh\ntouch {}\n", executed_path.display()),
    )
    .expect("probe script");
    make_executable(&probe_path);
    // D6: existing but non-executable file as first word stays native.
    let data_path = work_dir.join("data-1919.txt");
    std::fs::write(&data_path, "plain data\n").expect("data file");
    // Review counterproofs: dangling symlink and permission-opaque paths
    // are not provably ENOENT, so the missing-path branch must stand down
    // and leave the native 126/127 outcome untouched.
    let dangling_path = work_dir.join("dangling-1919");
    std::os::unix::fs::symlink(work_dir.join("no-such-target"), &dangling_path)
        .expect("dangling symlink");
    let opaque_dir = work_dir.join("opaque-1919");
    std::fs::create_dir_all(&opaque_dir).expect("opaque dir");
    let opaque_file = opaque_dir.join("real-file");
    std::fs::write(&opaque_file, "x\n").expect("opaque file");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&opaque_dir, std::fs::Permissions::from_mode(0o000))
            .expect("chmod opaque");
    }

    // UTF-8 locale for the same readline multi-byte reason as the positive
    // intercept case above.
    let mut config = ShellHostConfig::new("missing-path-native", &work_dir);
    config
        .env_overrides
        .push(("LANG".to_string(), "C.UTF-8".to_string()));
    config
        .env_overrides
        .push(("LC_ALL".to_string(), "C.UTF-8".to_string()));
    let existing_exec = probe_path.display().to_string();
    let existing_data = format!("{} 帮我读一下", data_path.display());
    let dangling_input = format!("{} 帮我读一下", dangling_path.display());
    let opaque_input = format!("{} 帮我读一下", opaque_file.display());
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::user_line(existing_exec.clone()),
            ScriptedInput::user_line("/usr/bin/nonexistent-cosh-1919-probe"),
            ScriptedInput::user_line(existing_data.clone()),
            ScriptedInput::user_line(dangling_input.clone()),
            ScriptedInput::user_line(opaque_input.clone()),
        ],
    )
    .expect("scripted bash pty");

    // I1: the existing script actually executed.
    assert!(
        executed_path.exists(),
        "existing script must run natively: {:?}",
        output.events
    );
    // No missing-path input may surface as a natural-language intercept.
    assert!(
        !output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.component.as_deref() == Some("natural_language")
        }),
        "{:?}",
        output.events
    );
    // I2: the plain-English typo path keeps the native bash error.
    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(terminal.contains("No such file or directory"), "{terminal}");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&opaque_dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore opaque");
    }
}
