use super::*;

fn run_raw_cli_ask_with_delayed_input(chunks: Vec<(Vec<u8>, Duration)>) -> String {
    run_raw_cli_ask_with_args_and_delayed_input(&[], chunks)
}

fn run_raw_cli_ask_with_args_and_delayed_input(
    args: &[&str],
    chunks: Vec<(Vec<u8>, Duration)>,
) -> String {
    run_raw_cli_ask_with_args_env_and_delayed_input(args, &[], chunks)
}

fn run_raw_cli_ask_with_args_env_and_delayed_input(
    args: &[&str],
    extra_env: &[(&str, &str)],
    chunks: Vec<(Vec<u8>, Duration)>,
) -> String {
    let home = temp_shell_home("approval-cards");
    write_cosh_config(
        &home,
        r#"approval.readonly_disabled = ["git status", "pwd"]"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let mut env = vec![("HOME", home_str.as_str())];
    env.extend_from_slice(extra_env);
    run_raw_cli_with_args_env_and_delayed_input("fake", args, &env, chunks)
}

#[path = "approval/details.rs"]
mod details;
#[path = "approval/foreground.rs"]
mod foreground;
#[path = "approval/input.rs"]
mod input;
#[path = "approval/slash_removed.rs"]
mod slash_removed;
#[path = "approval/status.rs"]
mod status;
