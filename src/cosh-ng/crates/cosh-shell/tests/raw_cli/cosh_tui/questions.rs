use super::*;

#[test]
fn raw_cli_cosh_tui_question_card_answer_continues_same_provider_turn() {
    let home = temp_shell_home("cosh-tui-question-answer");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-question","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-provider-question-card*)
    printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-question","message":{"content":[{"type":"tool_use","id":"toolu-cosh-tui-ask","name":"ask_user_question","input":{"question":"Choose a color for cosh-tui provider follow-up","options":[{"label":"Green"},{"label":"Blue"}],"allow_free_text":true}}]}}'
    printf '%s\n' '{"type":"control_request","request_id":"ask-cosh-tui-1","request":{"subtype":"ask_user","question":"Choose a color for cosh-tui provider follow-up","options":[{"label":"Green"},{"label":"Blue"}],"allow_free_text":true,"multi_select":false}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ask-cosh-tui-1"'*'"answer":"Green"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-question","message":{"content":[{"type":"text","text":"Cosh-tui question answer received in same provider turn."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-question","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-question","is_error":true,"result":"missing cosh-tui question answer"}'
    exit 1
    ;;
  *"Answer to pending Agent question"*)
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-question","is_error":true,"result":"question answer restarted provider instead of answering same turn"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-question","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[("HOME", &home_str), ("COSH_TUI_PATH", &cosh_tui_path_str)],
        vec![
            (
                b"?? cosh-tui-provider-question-card\n".to_vec(),
                Duration::ZERO,
            ),
            (b"\n".to_vec(), Duration::from_millis(1_200)),
            (b"exit\n".to_vec(), Duration::from_millis(1_500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    assert!(output.contains("Agent question"), "{output}");
    assert!(!output.contains("ask_user_question called"), "{output}");
    assert!(
        !output.contains("Tool called: ask_user_question"),
        "{output}"
    );
    assert!(
        output.contains("Choose a color for cosh-tui provider follow-up"),
        "{output}"
    );
    assert!(output.contains("[1] Green"), "{output}");
    assert!(output.contains("[2] Blue"), "{output}");
    assert!(output.contains("Answer: Green"), "{output}");
    assert!(
        output.contains("Cosh-tui question answer received in same provider turn."),
        "{output}"
    );
    assert!(
        !output.contains("missing cosh-tui question answer"),
        "{output}"
    );
    assert!(
        !output.contains("question answer restarted provider instead of answering same turn"),
        "{output}"
    );
    assert!(!output.contains("Got your answer:"), "{output}");
    assert!(!output.contains("/answer"), "{output}");
    assert!(!output.contains("Bash tool sent to shell"), "{output}");
    assert!(
        !output.contains("bash: cosh-tui-provider-question-card: command not found"),
        "{output}"
    );
    assert!(!output.contains("Agent timed out:"), "{output}");
}

#[test]
fn raw_cli_cosh_tui_narrow_question_and_debug_remain_readable() {
    let home = temp_shell_home("cosh-tui-narrow-question-debug");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let cosh_tui_path = bin_dir.join("cosh-tui");
    write_executable(
        &cosh_tui_path,
        r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-cosh-tui-narrow","model":"cosh-tui-test"}'
read -r user_message
case "$user_message" in
  *cosh-tui-narrow-question-debug*)
    printf '%s\n' '{"type":"control_request","request_id":"ask-cosh-tui-narrow-1","request":{"subtype":"ask_user","question":"Choose the narrow terminal follow-up action for cosh-tui provider output","options":[{"label":"Keep investigating"},{"label":"Open debug session"}],"allow_free_text":true,"multi_select":false}}'
    if IFS= read -r response; then
      case "$response" in
        *'"request_id":"ask-cosh-tui-narrow-1"'*'"answer":"Open debug session"'*)
          printf '%s\n' '{"type":"assistant","session_id":"sess-cosh-tui-narrow","message":{"content":[{"type":"text","text":"Cosh-tui narrow terminal answer received before debug."}]}}'
          printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-narrow","is_error":false,"result":"done"}'
          exit 0
          ;;
      esac
    fi
    printf '%s\n' '{"type":"result","subtype":"error","session_id":"sess-cosh-tui-narrow","is_error":true,"result":"missing narrow question answer"}'
    exit 1
    ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess-cosh-tui-narrow","is_error":false,"result":"ignored"}'
"#,
    );
    let home_str = home.to_string_lossy().to_string();
    let cosh_tui_path_str = cosh_tui_path.to_string_lossy().to_string();
    let output = run_raw_cli_with_args_env_and_delayed_input(
        "cosh-tui",
        &[],
        &[
            ("HOME", &home_str),
            ("COSH_TUI_PATH", &cosh_tui_path_str),
            ("TERM", "xterm-256color"),
            ("COSH_SHELL_WIDTH", "40"),
        ],
        vec![
            (
                b"?? cosh-tui-narrow-question-debug\n".to_vec(),
                Duration::ZERO,
            ),
            (
                b"Open debug session\n".to_vec(),
                Duration::from_millis(1_500),
            ),
            (b"/debug session\n".to_vec(), Duration::from_millis(1_500)),
            (b"exit\n".to_vec(), Duration::from_millis(500)),
        ],
    );
    let _ = fs::remove_dir_all(&home);

    let compact = compact_terminal_words(&output);
    assert!(output.contains("Agent question"), "{output}");
    assert!(output.contains("Choose the narrow terminal"), "{output}");
    assert!(output.contains("cosh-tui provider output"), "{output}");
    assert!(output.contains("[1] Keep investigating"), "{output}");
    assert!(output.contains("[2] Open debug session"), "{output}");
    assert!(compact.contains("Answer: Open debug session"), "{output}");
    assert!(
        output.contains("Cosh-tui narrow terminal answer"),
        "{output}"
    );
    assert!(output.contains("received before debug."), "{output}");
    assert!(output.contains("provider invocation:"), "{output}");
    assert!(
        !output.contains("missing narrow question answer"),
        "{output}"
    );
    assert!(!output.contains("bash: /debug"), "{output}");
    assert!(
        !output.contains("bash: cosh-tui-narrow-question-debug: command not found"),
        "{output}"
    );
    assert_agent_block_width(&output, 40);
}
