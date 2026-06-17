use super::*;

#[test]
fn raw_cli_zsh_native_loads_existing_user_history() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let home = temp_zsh_home("native-history");
    let history_file = home.join(".zsh_history");
    fs::write(
        home.join(".zshrc"),
        "HISTSIZE=1000\nSAVEHIST=1000\nsetopt appendhistory\n",
    )
    .unwrap();
    fs::write(&history_file, "echo old-cosh-zsh-history\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let history_str = history_file.to_string_lossy().to_string();

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[
            ("HOME", &home_str),
            ("TERM", "xterm-256color"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (
                b"printf 'histfile:%s\\n' \"$HISTFILE\"\n".to_vec(),
                Duration::ZERO,
            ),
            (b"history\n".to_vec(), Duration::from_millis(150)),
            (
                b"echo new-cosh-zsh-history\n".to_vec(),
                Duration::from_millis(150),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(150)),
        ],
    );

    assert!(
        output.contains(&format!("histfile:{history_str}")),
        "{output}"
    );
    assert!(output.contains("old-cosh-zsh-history"), "{output}");
    assert!(fs::read_to_string(&history_file)
        .unwrap()
        .contains("new-cosh-zsh-history"));
}

#[test]
fn raw_cli_bash_native_loads_existing_user_history() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let home = temp_shell_home("native-bash-history");
    let history_file = home.join(".bash_history");
    fs::write(
        home.join(".bashrc"),
        "export HISTFILE=$HOME/.bash_history\nexport HISTSIZE=1000\nshopt -s histappend\n",
    )
    .unwrap();
    fs::write(&history_file, "echo old-cosh-bash-history\n").unwrap();
    let home_str = home.to_string_lossy().to_string();
    let history_str = history_file.to_string_lossy().to_string();

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "bash"],
        &[
            ("HOME", &home_str),
            ("TERM", "xterm-256color"),
            ("COSH_SHELL_ISOLATED", "0"),
        ],
        vec![
            (
                b"printf 'histfile:%s\\n' \"$HISTFILE\"\n".to_vec(),
                Duration::ZERO,
            ),
            (b"history\n".to_vec(), Duration::from_millis(150)),
            (
                b"echo new-cosh-bash-history\n".to_vec(),
                Duration::from_millis(150),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(150)),
        ],
    );

    assert!(
        output.contains(&format!("histfile:{history_str}")),
        "{output}"
    );
    assert!(output.contains("old-cosh-bash-history"), "{output}");
    assert!(fs::read_to_string(&history_file)
        .unwrap()
        .contains("new-cosh-bash-history"));
}

#[test]
#[ignore = "native zsh completion can invoke user rc and real editor; keep out of default raw_cli"]
fn raw_cli_zsh_native_path_slash_and_tab_stay_in_shell() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("COSH_SHELL_ISOLATED", "0")],
        vec![
            (b"/Users".to_vec(), Duration::ZERO),
            (vec![0x03], Duration::from_millis(100)),
            (b"vim .".to_vec(), Duration::from_millis(100)),
            (b"/".to_vec(), Duration::from_millis(50)),
            (b"\t".to_vec(), Duration::from_millis(50)),
            (vec![0x03], Duration::from_millis(100)),
            (
                b"echo after-native-tab\n".to_vec(),
                Duration::from_millis(100),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("after-native-tab"), "{output}");
    assert!(!output.contains("Slash command hint"), "{output}");
    assert!(!output.contains("Slash commands"), "{output}");
    assert!(!output.contains("User mode"), "{output}");
    assert!(!output.contains("/mode [recommend|agent]"), "{output}");
}
