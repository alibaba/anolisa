use super::*;

#[test]
fn raw_cli_selects_recommendation_without_executing_it() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /explain last error\n\
         /select 2\n\
         echo after-select\n\
         exit 0\n",
    );

    assert!(output.contains("Recommendations"));
    assert!(output.contains("  1. pwd"));
    assert!(output.contains("  2. echo $PATH"));
    assert!(output.contains("Selected recommendation 2"));
    assert!(output.contains("echo $PATH"));
    assert!(output.contains("Display-only: command was not executed; copy or re-enter it to run"));
    assert!(output.contains("after-select"));
    assert!(!output.contains("/.cargo/bin"));
}

#[test]
fn raw_cli_zh_selects_recommendation_without_executing_it() {
    let output = run_raw_cli_with_env(
        "fake",
        "ls /path/that/does/not/exist\n\
         /explain last error\n\
         /select 2\n\
         echo after-select\n\
         exit 0\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("推荐"), "{output}");
    assert!(
        output.contains("[Copy] [Insert] [Details] - 仅展示"),
        "{output}"
    );
    assert!(output.contains("已选择推荐 2"), "{output}");
    assert!(output.contains("echo $PATH"), "{output}");
    assert!(output.contains("仅展示：命令未执行；复制或重新输入后才会运行"));
    assert!(output.contains("after-select"));
    assert!(!output.contains("/.cargo/bin"));
}

#[test]
fn raw_cli_copy_fallback_shows_recommendation_without_executing_it() {
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "fake",
        &[],
        &[("COSH_SHELL_LANG", "en-US")],
        vec![
            (b"ls /path/that/does/not/exist\n".to_vec(), Duration::ZERO),
            (
                b"/explain last error\n".to_vec(),
                Duration::from_millis(100),
            ),
            (b"/copy 1\n".to_vec(), Duration::from_millis(1_200)),
            (b"echo after-copy\n".to_vec(), Duration::from_millis(200)),
            (b"exit 0\n".to_vec(), Duration::from_millis(100)),
        ],
    );

    assert!(output.contains("Recommendation copy"));
    assert!(output.contains("Copy recommendation 1"));
    assert!(output.contains("pwd"));
    assert!(output.contains("Copy-only: command was shown for copying; it was not executed."));
    assert!(output.contains("after-copy"));
    assert!(!output.contains("bash: /copy"));
}

#[test]
fn raw_cli_select_before_recommendation_is_display_only_noop() {
    let output = run_raw_cli_with_input("fake", "/select 1\necho after-early-select\nexit\n");

    assert!(output.contains("No selectable recommendation is available yet"));
    assert!(output.contains("after-early-select"));
    assert!(!output.contains("The command ls "));
}

#[test]
fn raw_cli_zh_select_before_recommendation_uses_catalog_fallback() {
    let output = run_raw_cli_with_env(
        "fake",
        "/select 1\necho after-early-select\nexit\n",
        &[("COSH_SHELL_LANG", "zh-CN")],
    );

    assert!(output.contains("没有可选择的推荐"), "{output}");
    assert!(output.contains("当前还没有可选择的推荐"), "{output}");
    assert!(output.contains("after-early-select"), "{output}");
    assert!(!output.contains("No selectable recommendation"), "{output}");
    assert!(
        !output.contains("No selectable recommendation is available yet"),
        "{output}"
    );
    assert!(!output.contains("The command ls "), "{output}");
    assert_no_migrated_english_ui_labels(&output, RENDERER_ZH_FORBIDDEN_UI);
}

#[test]
fn raw_cli_select_out_of_range_uses_structured_notice() {
    let output = run_raw_cli_with_input(
        "fake",
        "ls /path/that/does/not/exist\n\
         /explain last error\n\
         /select 99\n\
         echo after-missing-select\n\
         exit\n",
    );

    assert!(output.contains("Recommendation unavailable"), "{output}");
    assert!(
        output.contains("Recommendation 99 is not available; choose 1..3"),
        "{output}"
    );
    assert!(output.contains("after-missing-select"), "{output}");
    assert!(!output.contains("bash: /select"), "{output}");
}
