use super::*;

#[test]
fn raw_cli_removed_draft_alias_falls_through_to_shell() {
    let home = temp_shell_home("agent-composer-removed-draft-alias");
    fs::write(home.join(".bashrc"), "PS1='alias-test$ '\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_current_dir_and_marker_input(
        "fake",
        &["--shell", "bash"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_LANG", "en-US"),
        ],
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &[
            ("alias-test$", b"/draft\n"),
            ("No such file or directory", b"exit 0\n"),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("bash: /draft"), "{output}");
    assert!(!output.contains("Agent Composer"), "{output}");
}

#[test]
fn raw_cli_bash_agent_composer_submits_multiline_request_and_restores_custom_prompt() {
    let home = temp_shell_home("agent-composer-bash");
    fs::write(home.join(".bashrc"), "PS1='alice@remote:\\w$ '\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = run_raw_cli_with_args_env_current_dir_and_marker_input(
        "fake",
        &["--shell", "bash"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_LANG", "en-US"),
        ],
        cwd,
        &[
            ("alice@remote:", b"/agent\n"),
            (
                "Agent Composer",
                b"/skill:repo-review inspect @Cargo.toml\x1b[13;2uand @src\r",
            ),
            ("Received shell prompt request:", b"echo after-composer\n"),
            ("after-composer", b"exit\n"),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Runtime: fake"), "{output}");
    assert!(
        output.contains("/skill:repo-review inspect @Cargo.toml"),
        "{output}"
    );
    assert!(output.contains("and @src"), "{output}");
    assert!(output.contains("after-composer"), "{output}");
    assert!(count_occurrences(&output, "alice@remote:") >= 2, "{output}");
    assert!(!output.contains("bash: /agent"), "{output}");
    let composer = output.find("Agent Composer").expect("composer card");
    let draft_text = output[composer..]
        .find("/skill:repo-review")
        .map(|offset| composer + offset)
        .expect("composer input");
    assert!(
        !strip_ansi_escape(&output[composer..draft_text]).contains("alice@remote:"),
        "the shell prompt must stay hidden while the composer owns input: {output}"
    );
}

#[test]
fn raw_cli_agent_composer_suggests_and_accepts_workspace_paths() {
    let home = temp_shell_home("agent-composer-path-completion");
    fs::write(home.join(".bashrc"), "PS1='alice@remote:\\w$ '\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = run_raw_cli_with_args_env_current_dir_and_marker_input(
        "fake",
        &["--shell", "bash"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_LANG", "en-US"),
        ],
        cwd,
        &[
            ("alice@remote:", b"/agent\n"),
            ("Agent Composer", b"review @Car"),
            ("› @Cargo.toml", b"\tinspect\x1b"),
            ("Draft cancelled", b"exit\n"),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("› @Cargo.toml"), "{output}");
    assert!(output.contains("review @Cargo.toml inspect"), "{output}");
}

#[test]
fn raw_cli_zsh_agent_composer_cancel_restores_custom_prompt() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_zsh_home("agent-composer-zsh");
    fs::write(
        home.join(".zshrc"),
        "PROMPT='zsh@remote:%~%# '\nRPROMPT=''\n",
    )
    .unwrap();
    let home_str = home.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_current_dir_and_marker_input(
        "fake",
        &["--shell", "zsh"],
        &[
            ("HOME", &home_str),
            ("COSH_SHELL_STARTUP_BANNER", "0"),
            ("COSH_SHELL_ISOLATED", "0"),
            ("COSH_SHELL_LANG", "en-US"),
        ],
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &[
            ("zsh@remote:", b"/agent\n"),
            ("Agent Composer", b"cancel this draft\x1b"),
            ("Draft cancelled", b"echo after-cancel\n"),
            ("after-cancel", b"exit\n"),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Runtime: fake"), "{output}");
    assert!(output.contains("Draft cancelled"), "{output}");
    assert!(output.contains("after-cancel"), "{output}");
    assert!(count_occurrences(&output, "zsh@remote:") >= 2, "{output}");
    assert!(
        !output.contains("Received shell prompt request: cancel this draft"),
        "{output}"
    );
}
