use super::*;

#[test]
fn raw_cli_help_renders_slash_command_reference() {
    let output = run_raw_cli_with_input("fake", "/help\necho after-help\nexit\n");

    assert!(output.contains("Slash commands"), "{output}");
    assert!(output.contains("Config"), "{output}");
    assert!(output.contains("Modes"), "{output}");
    assert!(output.contains("Hooks"), "{output}");
    assert!(output.contains("Registry"), "{output}");
    assert!(!output.contains("Inspect"), "{output}");
    assert!(!output.contains("Recommendations"), "{output}");
    assert!(
        output.contains("/config language [auto|en-US|zh-CN]"),
        "{output}"
    );
    assert!(
        output.contains("/mode approval [recommend|auto|trust]"),
        "{output}"
    );
    assert!(
        output.contains("/mode analysis [smart|auto|manual]"),
        "{output}"
    );
    assert!(
        output.contains("/extensions [list|detail] [name]"),
        "{output}"
    );
    assert!(output.contains("/skills [list|detail] [name]"), "{output}");
    assert!(!output.contains("/agent"), "{output}");
    assert!(!output.contains("/explain"), "{output}");
    assert!(!output.contains("/cancel"), "{output}");
    assert!(!output.contains("/details"), "{output}");
    assert!(!output.contains("command_id"), "{output}");
    assert!(!output.contains("output_id"), "{output}");
    assert!(!output.contains("insight_id"), "{output}");
    assert!(!output.contains("/audit"), "{output}");
    assert!(!output.contains("/select N"), "{output}");
    assert!(!output.contains("/copy N"), "{output}");
    assert!(!output.contains("/mode [recommend|auto|trust]"), "{output}");
    assert!(
        !output.contains("/approval-mode [suggest|ask|auto|trust]"),
        "{output}"
    );
    assert!(!output.contains("advanced legacy governance"), "{output}");
    assert!(!output.contains("/allow <n>"), "{output}");
    assert!(!output.contains("[ask|auto]alias"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash commands"), "{output}");
    assert!(output.contains("Mode: auto."), "{output}");
    assert!(output.contains("after-help"), "{output}");
    assert!(!output.contains("bash: /help"), "{output}");
}

#[test]
fn raw_cli_unknown_slash_suggests_nearest_canonical_command() {
    let output = run_raw_cli_with_input("fake", "/hep\necho after-unknown\nexit\n");

    assert!(output.contains("Unknown slash command: /hep"), "{output}");
    assert!(output.contains("Did you mean /help?"), "{output}");
    assert!(!output.contains("/approval-mode"), "{output}");
    assert!(output.contains("after-unknown"), "{output}");
    assert!(!output.contains("bash: /hep"), "{output}");
}

#[test]
fn raw_cli_unknown_slash_uses_zh_language_env() {
    let output = run_raw_cli_with_env(
        "fake",
        "/hep\n\
         echo after-unknown-zh\n\
         exit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("未知 slash 命令: /hep"), "{output}");
    assert!(output.contains("你是不是想用 /help？"), "{output}");
    assert!(output.contains("使用 /help 查看可用命令。"), "{output}");
    assert!(!output.contains("Unknown slash command"), "{output}");
    assert!(!output.contains("Did you mean /help?"), "{output}");
    assert!(
        !output.contains("Use /help to see available commands."),
        "{output}"
    );
    assert!(output.contains("after-unknown-zh"), "{output}");
    assert!(!output.contains("bash: /hep"), "{output}");
    assert_no_migrated_english_ui_labels(&output, SLASH_CONFIG_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_informational_slash_commands_render_feedback() {
    let output = run_raw_cli_with_input(
        "fake",
        "/extensions\n\
         /config\n\
         /audit\n\
         echo after-info-slash\n\
         exit\n",
    );

    // /extensions with fake adapter shows degradation message
    assert!(
        output.contains("cosh-core") || output.contains("后端"),
        "{output}"
    );
    assert!(output.contains("Config"), "{output}");
    assert!(output.contains("language:"), "{output}");
    assert!(output.contains("debug activity: off"), "{output}");
    assert!(output.contains("Use /config language"), "{output}");
    assert!(output.contains("Audit"), "{output}");
    assert!(
        output.contains("Approval decisions are available with Details actions"),
        "{output}"
    );
    assert!(output.contains("after-info-slash"), "{output}");
    assert!(!output.contains("bash: /skill"), "{output}");
    assert!(!output.contains("bash: /config"), "{output}");
    assert!(!output.contains("bash: /audit"), "{output}");
}

#[test]
fn raw_cli_bare_slash_is_noop_without_hint_card() {
    let output = run_raw_cli_with_delayed_input(
        "fake",
        vec![
            (b"/\n".to_vec(), Duration::ZERO),
            (
                b"echo after-bare-slash\n".to_vec(),
                Duration::from_millis(200),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(!output.contains("Slash command hint"), "{output}");
    assert!(!output.contains("/help  /mode"), "{output}");
    assert!(!output.contains("bash: /"), "{output}");
    assert!(output.contains("after-bare-slash"), "{output}");
}

#[test]
fn raw_cli_slash_prefix_renders_hint_without_leaking_to_shell() {
    let output = run_raw_cli_with_input(
        "fake",
        "/mo\n\
         echo after-slash-hint\n\
         exit\n",
    );

    assert!(output.contains("Slash command hint"), "{output}");
    assert!(
        output.contains("/mode approval [recommend|auto|trust] - change approval mode"),
        "{output}"
    );
    assert!(!output.contains("/allow <n>"), "{output}");
    assert!(output.contains("Prefix: /mo"), "{output}");
    assert!(output.contains("after-slash-hint"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash command"), "{output}");
    assert!(!output.contains("bash: /:"), "{output}");
    assert!(!output.contains("bash: /mo"), "{output}");
}

#[test]
fn raw_cli_slash_cards_wrap_long_text_and_restore_prompt() {
    let output = run_raw_cli_with_env(
        "fake",
        "/help\n\
         echo after-long-slash\n\
         exit\n",
        &[("TERM", "xterm-256color"), ("COSH_SHELL_WIDTH", "72")],
    );

    assert!(output.contains("Slash commands"), "{output}");
    assert!(
        output.contains("/mode approval [recommend|auto|trust]"),
        "{output}"
    );
    assert!(output.contains("change approval mode"), "{output}");
    assert!(output.contains("after-long-slash"), "{output}");
    assert_agent_block_width(&output, 72);
    assert!(!output.contains("[ask|auto]alias"), "{output}");
    assert!(!output.contains("cosh-osc$ ╭ Slash"), "{output}");
    assert!(!output.contains("bash: /asdf"), "{output}");
}

#[test]
fn raw_cli_zsh_shell_arg_intercepts_fragmented_slash() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &["--shell", "zsh"],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"/he".to_vec(), Duration::ZERO),
            (b"lp\n".to_vec(), Duration::from_millis(50)),
            (
                b"echo after-zsh-slash\n".to_vec(),
                Duration::from_millis(100),
            ),
            (b"exit\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("Slash commands"), "{output}");
    assert!(
        output.contains("/mode approval [recommend|auto|trust]"),
        "{output}"
    );
    assert!(output.contains("after-zsh-slash"), "{output}");
    assert!(
        !output.contains("zsh: no such file or directory: /help"),
        "{output}"
    );
}
