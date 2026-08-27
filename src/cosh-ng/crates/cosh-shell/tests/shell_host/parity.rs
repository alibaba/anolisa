use super::*;

fn bash_available() -> bool {
    match Command::new("bash").arg("--version").output() {
        Ok(_) => true,
        Err(error) => {
            eprintln!("SKIP: bash is unavailable: {error}");
            false
        }
    }
}

#[test]
fn enhanced_bash_errexit_context_keeps_interactive_session_alive() {
    if !bash_available() {
        return;
    }

    for login_shell in [false, true] {
        let mode = if login_shell { "login" } else { "nonlogin" };
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-shell-errexit-{mode}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut config = ShellHostConfig::new(format!("errexit-{mode}"), &work_dir)
            .with_integration(ShellIntegration::Enhanced)
            .with_env("LANG", "C.UTF-8")
            .with_env("LC_ALL", "C.UTF-8");
        config.login_shell = login_shell;

        let output = run_scripted_bash(
            &config,
            &[
                ScriptedInput::command(
                    "{ set -e; false && printf bad; printf '__ERREXIT_CONTEXT__\\n'; }",
                ),
                ScriptedInput::command("printf '__SESSION_CONTINUED__\\n'"),
            ],
        )
        .unwrap_or_else(|error| panic!("{mode} scripted bash: {error}"));

        let terminal = String::from_utf8_lossy(&output.terminal_output);
        assert!(
            terminal.contains("__ERREXIT_CONTEXT__"),
            "{mode}: {terminal}"
        );
        assert!(
            terminal.contains("__SESSION_CONTINUED__"),
            "{mode}: {terminal}"
        );
        assert_eq!(output.exit_status, Some(0), "{mode}: {terminal}");

        let context = output
            .events
            .iter()
            .find(|event| {
                event.kind == ShellEventKind::CommandCompleted
                    && event.command.as_deref().is_some_and(|command| {
                        command.contains("false && printf bad")
                            && command.contains("__ERREXIT_CONTEXT__")
                    })
            })
            .unwrap_or_else(|| panic!("{mode}: missing errexit completion: {:?}", output.events));
        assert_eq!(context.exit_code, Some(0), "{mode}: {terminal}");
        assert!(
            output.events.iter().any(|event| {
                event.kind == ShellEventKind::CommandCompleted
                    && event.command.as_deref() == Some("printf '__SESSION_CONTINUED__\\n'")
                    && event.exit_code == Some(0)
            }),
            "{mode}: missing continuation completion: {:?}",
            output.events
        );

        let _ = std::fs::remove_dir_all(&work_dir);
    }
}

#[test]
fn enhanced_bash_preserves_last_argument_across_prompt_boundary() {
    if !bash_available() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-last-argument-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("last-argument", &work_dir)
        .with_integration(ShellIntegration::Enhanced)
        .with_env("LANG", "C.UTF-8")
        .with_env("LC_ALL", "C.UTF-8");
    let output = run_scripted_bash(
        &config,
        &[
            ScriptedInput::command("echo hello world"),
            ScriptedInput::command("printf '__LAST_ARGUMENT__=[%s]\\n' \"$_\""),
        ],
    )
    .expect("scripted bash");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(terminal.contains("__LAST_ARGUMENT__=[world]"), "{terminal}");

    let _ = std::fs::remove_dir_all(&work_dir);
}

#[test]
fn enhanced_bash_keeps_internal_prompt_hooks_out_of_child_env() {
    if !bash_available() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-command-env-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("prompt-command-env", &work_dir)
        .with_integration(ShellIntegration::Enhanced)
        .with_env("PROMPT_COMMAND", "printf ''")
        .with_env("LANG", "C.UTF-8")
        .with_env("LC_ALL", "C.UTF-8");
    let output = run_scripted_bash(
        &config,
        &[ScriptedInput::command(
            "printf '__PROMPT_COMMAND__=%s\\n' \"$(declare -p PROMPT_COMMAND)\"; \
             if env | grep -q '^PROMPT_COMMAND='; then \
               printf '__CHILD_PROMPT_COMMAND__=present\\n'; \
             else \
               printf '__CHILD_PROMPT_COMMAND__=absent\\n'; \
             fi; \
             (PROMPT_COMMAND=('printf p1' 'printf p2'); \
             printf '__REASSIGNED_PROMPT_COMMAND__=%s\\n' \
             \"$(declare -p PROMPT_COMMAND)\")",
        )],
    )
    .expect("scripted bash");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    let expected = if bash_supports_prompt_command_array() {
        "__PROMPT_COMMAND__=declare -ax PROMPT_COMMAND="
    } else {
        "__PROMPT_COMMAND__=declare -- PROMPT_COMMAND="
    };
    assert!(terminal.contains(expected), "{terminal}");
    assert!(
        terminal.contains("__CHILD_PROMPT_COMMAND__=absent"),
        "{terminal}"
    );
    let reassigned = if bash_supports_prompt_command_array() {
        "__REASSIGNED_PROMPT_COMMAND__=declare -ax PROMPT_COMMAND="
    } else {
        "__REASSIGNED_PROMPT_COMMAND__=declare -a PROMPT_COMMAND="
    };
    assert!(terminal.contains(reassigned), "{terminal}");

    let _ = std::fs::remove_dir_all(&work_dir);
}

#[test]
fn enhanced_bash_keeps_three_background_jobs_concurrent() {
    if !bash_available() {
        return;
    }

    for login_shell in [false, true] {
        let mode = if login_shell { "login" } else { "nonlogin" };
        let work_dir = std::env::temp_dir().join(format!(
            "cosh-shell-background-parity-{mode}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let results = shell_arg(&work_dir.join("background-results"));
        let release = shell_arg(&work_dir.join("background-release"));
        let command = format!(
            "rm -f {results} {release}; \
             for i in 1 2 3; do \
               (sleep 0.3; while [[ ! -e {release} ]]; do sleep 0.05; done; \
                printf 'h%s\\n' \"$i\" >> {results}) & \
             done; \
             third=$!; pids=($(jobs -p)); jobs -r; \
             if [[ \"$third\" == \"${{pids[2]-}}\" ]]; then \
               printf '__LAST_IS_THIRD__=yes\\n'; \
             else \
               printf '__LAST_IS_THIRD__=no\\n'; \
             fi; \
             : > {release}; wait; sort {results}"
        );
        let mut config = ShellHostConfig::new(format!("background-parity-{mode}"), &work_dir)
            .with_integration(ShellIntegration::Enhanced)
            .with_env("LANG", "C.UTF-8")
            .with_env("LC_ALL", "C.UTF-8");
        config.login_shell = login_shell;
        let output = run_scripted_bash(&config, &[ScriptedInput::command(command.clone())])
            .unwrap_or_else(|error| panic!("{mode} scripted bash: {error}"));

        let ledger = ledger_from_output(&output);
        let block = ledger
            .blocks
            .iter()
            .find(|block| block.command == command)
            .unwrap_or_else(|| panic!("{mode}: missing background command: {:#?}", ledger.blocks));
        assert_eq!(block.exit_code, 0, "{mode}");
        let output_ref = block
            .output
            .terminal_output_ref
            .as_deref()
            .unwrap_or_else(|| panic!("{mode}: background output ref"));
        let command_output = std::fs::read_to_string(output_ref)
            .unwrap_or_else(|error| panic!("{mode}: background output: {error}"));

        for job in ["[1]", "[2]", "[3]"] {
            assert!(command_output.contains(job), "{mode}: {command_output}");
        }
        assert_eq!(
            command_output
                .lines()
                .filter(|line| line.contains("Running"))
                .count(),
            3,
            "{mode}: {command_output}"
        );
        assert!(
            command_output.contains("__LAST_IS_THIRD__=yes"),
            "{mode}: {command_output}"
        );
        let lines = command_output.lines().map(str::trim).collect::<Vec<_>>();
        for result in ["h1", "h2", "h3"] {
            assert!(lines.contains(&result), "{mode}: {command_output}");
        }

        let _ = std::fs::remove_dir_all(&work_dir);
    }
}
