use super::*;

#[test]
fn raw_cli_approval_text_input_does_not_confirm_or_leak_to_bash() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"exit\n".to_vec(), Duration::from_millis(5_000)),
        (b"\x1b".to_vec(), Duration::from_millis(200)),
        (b"exit\n".to_vec(), Duration::from_millis(200)),
    ]);

    assert_approval_prompt_visible(&output);
    assert!(output.contains("tool request") && output.contains("medium risk"));
    assert!(output.contains("Cancelled"));
    assert!(output.contains("$ git status"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("tool request - cancelled by user"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("Decision: approved"));
    assert_eq!(count_occurrences(&output, "cosh-osc$ exit"), 1, "{output}");
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approval_split_arrow_sequence_does_not_cancel() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1b".to_vec(), Duration::from_millis(5_000)),
        (b"[".to_vec(), Duration::from_millis(50)),
        (b"C".to_vec(), Duration::from_millis(50)),
        (b"\x1b[C".to_vec(), Duration::from_millis(50)),
        (b"\n".to_vec(), Duration::from_millis(100)),
        (b"exit\n".to_vec(), Duration::from_millis(1_000)),
    ]);

    assert_approval_prompt_visible(&output);
    assert!(output.contains("tool request") && output.contains("medium risk"));
    assert!(
        output.contains("> [ Deny ]") || output.contains("[Deny]"),
        "{output}"
    );
    assert!(output.contains("Denied"));
    assert!(output.contains("$ git status --short"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("Bash tool - denied"));
    assert!(!output.contains("Cancelled"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("bash:"));
}

#[test]
fn raw_cli_approval_application_cursor_arrow_updates_focus() {
    let output = run_raw_cli_ask_with_delayed_input(vec![
        (b"?? stream tool approval\n".to_vec(), Duration::ZERO),
        (b"\x1bOC".to_vec(), Duration::from_millis(5_000)),
        (b"\x1bOC".to_vec(), Duration::from_millis(100)),
        (b"\n".to_vec(), Duration::from_millis(100)),
        (b"exit\n".to_vec(), Duration::from_millis(1_000)),
    ]);

    assert_approval_prompt_visible(&output);
    assert!(output.contains("tool request") && output.contains("medium risk"));
    assert!(
        output.contains("> [ Deny ]") || output.contains("[Deny]"),
        "{output}"
    );
    assert!(output.contains("Denied"));
    assert!(output.contains("$ git status --short"));
    assert!(!output.contains("No command ran."));
    assert!(!output.contains("Bash tool - denied"));
    assert!(!output.contains("Cancelled"));
    assert!(!output.contains("Approved"));
    assert!(!output.contains("bash:"));
}
