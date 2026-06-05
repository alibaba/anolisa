use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::Command;
use std::time::Duration;

use cosh_shell::{
    adapter_for_kind, agent_request_after_confirmation, build_command_blocks, findings_from_blocks,
    govern_agent_events, read_shell_events, run_line_interactive_bash, run_raw_relay_bash,
    run_raw_relay_bash_with_actions, run_raw_relay_bash_with_actions_output_control,
    run_raw_relay_bash_with_observer, run_raw_relay_zsh_with_actions, run_scripted_bash,
    run_scripted_zsh, AdapterKind, AgentAdapter, AgentEvent, GovernanceDecision, Policy,
    RawObserverAction, RawRelayAction, ScriptedInput, ShellEventKind, ShellHostConfig,
};

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
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("please explain the last error")
            && event.component.as_deref() == Some("natural_language")
    }));
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
    let output_ref = failed
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_ref_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(output_ref_text.contains("No such file") || output_ref_text.contains("cannot access"));
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
}

#[test]
fn raw_relay_zsh_adapter_uses_shared_event_contract() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-raw-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let unicode_file = work_dir.join("\u{8bbe}\u{8ba1}\u{6587}\u{6863}.md");
    std::fs::write(&unicode_file, "\u{4e2d}\u{6587}\u{5185}\u{5bb9}").expect("unicode file");

    let config = ShellHostConfig::new("zsh-raw-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::line("/help"),
            RawRelayAction::line("echo zsh-raw-ok"),
            RawRelayAction::line(format!("cat {}", shell_arg(&unicode_file))),
            RawRelayAction::line("ls /path/that/does/not/exist"),
        ],
        &mut rendered,
    )
    .expect("raw zsh relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("zsh-raw-ok"), "{rendered_text}");
    assert!(
        rendered_text.contains("\u{4e2d}\u{6587}\u{5185}\u{5bb9}"),
        "{rendered_text}"
    );
    assert_no_osc_marker(&rendered);
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/help")
            && event.component.as_deref() == Some("slash")
    }));

    let ledger = ledger_from_output(&output);
    let echo_block = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("echo zsh-raw-ok") && block.exit_code == 0)
        .expect("zsh echo command block");
    assert_clean_shell_output_ref(echo_block, "zsh-raw-ok");
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("cat ") && block.exit_code == 0));
    assert!(ledger.blocks.iter().any(|block| {
        block.command.contains("/path/that/does/not/exist") && block.exit_code != 0
    }));
}

#[test]
fn raw_relay_zsh_buffers_fragmented_intercept_candidates() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-fragment-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    let config = ShellHostConfig::new("zsh-fragment-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::write("/he"),
            RawRelayAction::write("lp\n"),
            RawRelayAction::write("\u{4f60}".as_bytes()),
            RawRelayAction::write("\u{597d}\n".as_bytes()),
            RawRelayAction::write("?? zsh "),
            RawRelayAction::write("fragmented agent\n"),
            RawRelayAction::write("?? zsh combined agent\necho after-zsh-combined\n"),
            RawRelayAction::line("echo after-zsh-fragment"),
        ],
        &mut rendered,
    )
    .expect("raw zsh fragmented relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(
        rendered_text.contains("after-zsh-fragment"),
        "{rendered_text}"
    );
    assert!(
        rendered_text.contains("after-zsh-combined"),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("zsh: no such file or directory: /help"));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("/help")
            && event.component.as_deref() == Some("slash")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("\u{4f60}\u{597d}")
            && event.component.as_deref() == Some("natural_language")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("?? zsh fragmented agent")
            && event.component.as_deref() == Some("agent_marker")
    }));
    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("?? zsh combined agent")
            && event.component.as_deref() == Some("agent_marker")
    }));
}

#[test]
fn raw_relay_bash_renders_inline_slash_completion_while_typing() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-slash-completion-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    let config = ShellHostConfig::new("slash-completion-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"/".to_vec()),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"mo".to_vec()),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(b"de auto\n".to_vec()),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
        |_, _| Ok(RawObserverAction::Continue),
    )
    .expect("raw bash slash completion");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("cosh-osc$ /"), "{rendered_text}");
    assert!(
        !rendered_text.contains("cosh-osc$ /  /help  /mode  /details  /skill"),
        "{rendered_text}"
    );
    assert!(
        rendered_text.contains("cosh-osc$ /mo\x1b[s\x1b[2m  /mode [ask|auto]\x1b[0m\x1b[u"),
        "{rendered_text}"
    );
    assert!(
        output.events.iter().any(|event| {
            event.kind == ShellEventKind::UserInputIntercepted
                && event.input.as_deref() == Some("/mode auto")
                && event.component.as_deref() == Some("slash")
        }),
        "{rendered_text}\n{:?}",
        output.events
    );
    assert!(!rendered_text.contains("bash: /mode"), "{rendered_text}");
}

#[test]
fn raw_relay_zsh_preserves_session_history() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-zsh-history-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");

    let config = ShellHostConfig::new("zsh-history-test", &work_dir);
    let mut rendered = Vec::new();
    run_raw_relay_zsh_with_actions(
        &config,
        vec![
            RawRelayAction::line("pwd"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("history"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("ls -ltrh"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("history"),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("exit"),
        ],
        &mut rendered,
    )
    .expect("raw zsh history");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("    1  pwd"), "{rendered_text}");
    assert!(
        rendered_text.contains("    3  ls -ltrh") || rendered_text.contains("    2  ls -ltrh"),
        "{rendered_text}"
    );
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn ledger_from_output(output: &cosh_shell::ShellHostOutput) -> cosh_shell::LedgerOutput {
    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    ledger
}

fn assert_no_osc_marker(output: &[u8]) {
    assert!(!output
        .windows(b"\x1b]1337;COSH;".len())
        .any(|window| window == b"\x1b]1337;COSH;"));
}

fn assert_clean_shell_output_ref(block: &cosh_shell::CommandBlock, expected: &str) {
    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(text.contains(expected), "{text:?}");
    assert!(!text.contains("\x1b[?2004"), "{text:?}");
    assert!(!text.contains('\u{0008}'), "{text:?}");
    assert!(!text.contains("\x1b[0m"), "{text:?}");
    assert!(!text.contains("\x1b[27m"), "{text:?}");
    assert!(!text.contains("\x1b[24m"), "{text:?}");
    assert!(!text.contains("\x1b[J"), "{text:?}");
    assert!(!text.contains("\x1b[K"), "{text:?}");
}

#[test]
fn raw_relay_hold_mode_drops_input_without_writing_to_bash() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-hold-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("hold-test", &work_dir);
    let mut observer_calls = 0usize;
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("echo should-not-run"),
        ],
        Vec::new(),
        move |_, _| {
            observer_calls += 1;
            if observer_calls < 20 {
                Ok(RawObserverAction::HoldShellOutput)
            } else {
                Ok(RawObserverAction::Continue)
            }
        },
    )
    .expect("raw relay hold mode");

    let terminal = String::from_utf8_lossy(&output.terminal_output);
    assert!(!terminal.contains("should-not-run"), "{terminal}");
    let ledger = ledger_from_output(&output);
    assert!(!ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("should-not-run")));
}

#[test]
fn raw_relay_hold_mode_still_observes_ctrl_c() {
    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-hold-ctrl-c-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("hold-ctrl-c-test", &work_dir);
    let mut observer_calls = 0usize;
    let output = run_raw_relay_bash_with_actions_output_control(
        &config,
        vec![
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::write(vec![0x03]),
        ],
        Vec::new(),
        move |_, _| {
            observer_calls += 1;
            if observer_calls < 20 {
                Ok(RawObserverAction::HoldShellOutput)
            } else {
                Ok(RawObserverAction::Continue)
            }
        },
    )
    .expect("raw relay hold ctrl-c");

    assert!(output.events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("control")
            && event.input.as_deref() == Some("ctrl_c")
    }));
}

fn shell_arg(path: &std::path::Path) -> String {
    let value = path.display().to_string();
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[test]
fn line_interactive_host_routes_input_to_bash_and_journal() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-line-host-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("line-host-test", &work_dir);
    let input = std::io::Cursor::new(
        "/explain last error\n\
         echo line-ok\n\
         please explain the last error\n\
         ls /path/that/does/not/exist\n",
    );
    let mut rendered = Vec::new();
    let output =
        run_line_interactive_bash(&config, input, &mut rendered).expect("line interactive host");

    let rendered_text = String::from_utf8_lossy(&output.rendered_output);
    assert!(!rendered_text.contains("intercepted  slash"));
    assert!(!rendered_text.contains("intercepted  natural_language"));
    assert!(rendered_text.contains("line-ok"));

    let replayed_events = read_shell_events(&output.shell.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo line-ok") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("/path/that/does/not/exist") && block.exit_code != 0));
}

#[test]
fn line_interactive_host_can_invoke_claude_adapter_through_governance() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-line-claude-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("line-claude-test", &work_dir);
    let input = std::io::Cursor::new(
        "/explain last error\n\
         ls /path/that/does/not/exist\n",
    );
    let mut rendered = Vec::new();
    let output =
        run_line_interactive_bash(&config, input, &mut rendered).expect("line interactive host");

    let replayed_events = read_shell_events(&output.shell.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);

    let failed = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("/path/that/does/not/exist"))
        .expect("failed command block");
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("line-claude-test", failed, &findings, true)
        .expect("confirmed request");

    let agent_events = adapter_for_kind(AdapterKind::ClaudeCode)
        .run(&request)
        .expect("claude dry-run adapter");
    assert!(agent_events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. } if text.contains("claude --print")
    )));

    let governed = govern_agent_events(&agent_events, &Policy::default());
    assert!(governed.events.iter().all(|event| !event.auto_execute));
}

#[test]
fn raw_relay_host_forwards_ctrl_c_and_keeps_shell_usable() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-relay-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-relay-test", &work_dir);
    let input = DelayedInput::new(vec![
        (b"sleep 5\n".to_vec(), Duration::ZERO),
        (vec![0x03], Duration::from_millis(250)),
        (
            b"echo after-ctrl-c\nls /path/that/does/not/exist\nexit\n".to_vec(),
            Duration::from_millis(100),
        ),
    ]);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash(&config, input, &mut rendered).expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("after-ctrl-c"));
    assert!(!rendered
        .windows(b"\x1b]1337;COSH;".len())
        .any(|window| window == b"\x1b]1337;COSH;"));

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    assert!(replayed_events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("control")
            && event.input.as_deref() == Some("ctrl_c")
    }));
    assert!(replayed_events
        .iter()
        .any(|event| event.kind == ShellEventKind::ShellExited));
}

#[test]
fn raw_relay_host_keeps_background_command_continuity() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-background-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-background-test", &work_dir);
    let input = std::io::Cursor::new(
        "sleep 0.2 &\n\
         echo after-background\n\
         exit\n",
    );
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash(&config, input, &mut rendered).expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("after-background"));

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("sleep 0.2 &") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-background") && block.exit_code == 0));
}

#[test]
fn raw_relay_host_applies_resize_actions() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-resize-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-resize-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::resize(40, 100),
            RawRelayAction::wait(Duration::from_millis(50)),
            RawRelayAction::line("stty size"),
        ],
        &mut rendered,
    )
    .expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("40 100"), "{rendered_text}");

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("stty size"))
        .expect("stty size command block");
    assert_eq!(block.exit_code, 0);
    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_ref_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(output_ref_text.contains("40 100"), "{output_ref_text}");
}

#[test]
fn raw_relay_preserves_terminal_control_sequences_but_cleans_output_ref() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-display-control-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-display-control-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![RawRelayAction::line("printf 'before\\033[Kafter\\n'")],
        &mut rendered,
    )
    .expect("raw relay host");

    assert!(
        rendered
            .windows(b"\x1b[K".len())
            .any(|window| window == b"\x1b[K"),
        "{:?}",
        String::from_utf8_lossy(&rendered)
    );

    let ledger = ledger_from_output(&output);
    let block = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("printf 'before"))
        .expect("printf block");
    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_ref_bytes = std::fs::read(output_ref).expect("output ref bytes");
    assert!(!output_ref_bytes
        .windows(b"\x1b[K".len())
        .any(|window| window == b"\x1b[K"));
    assert!(
        String::from_utf8_lossy(&output_ref_bytes).contains("beforeafter"),
        "{:?}",
        String::from_utf8_lossy(&output_ref_bytes)
    );
}

#[test]
fn raw_relay_host_runs_fullscreen_programs_and_keeps_shell_usable() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-fullscreen-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let vim_file = work_dir.join("vim.txt");
    std::fs::write(&vim_file, "").expect("vim file");
    let config = ShellHostConfig::new("raw-fullscreen-test", &work_dir);

    let has_less = Command::new("less").arg("--version").output().is_ok();
    let has_vim = Command::new("vim").arg("--version").output().is_ok();

    let mut actions = Vec::new();

    if has_less {
        actions.push(RawRelayAction::line("seq 1 200 | less"));
        actions.push(RawRelayAction::wait(Duration::from_millis(300)));
        actions.push(RawRelayAction::write(b"q".to_vec()));
        actions.push(RawRelayAction::line("echo after-less"));
    }

    if has_vim {
        actions.push(RawRelayAction::line(format!(
            "vim -Nu NONE -n {}",
            shell_arg(&vim_file)
        )));
        actions.push(RawRelayAction::wait(Duration::from_millis(500)));
        actions.push(RawRelayAction::write(b"\x1b:q!\n".to_vec()));
        actions.push(RawRelayAction::wait(Duration::from_millis(100)));
        actions.push(RawRelayAction::line("echo after-vim"));
    }

    if actions.is_empty() {
        return;
    }

    let mut rendered = Vec::new();
    let output =
        run_raw_relay_bash_with_actions(&config, actions, &mut rendered).expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert_no_osc_marker(&rendered);

    if has_less {
        assert!(rendered_text.contains("after-less"), "{rendered_text}");
    }
    if has_vim {
        assert!(rendered_text.contains("after-vim"), "{rendered_text}");
    }

    let ledger = ledger_from_output(&output);
    if has_less {
        assert!(ledger
            .blocks
            .iter()
            .any(|block| block.command.contains("seq 1 200 | less") && block.exit_code == 0));
    }
    if has_vim {
        assert!(ledger
            .blocks
            .iter()
            .any(|block| block.command.contains("vim -Nu NONE -n") && block.exit_code == 0));
    }
}

#[test]
fn line_interactive_host_runs_shell_command_with_non_ascii_path() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-line-unicode-path-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let file_name = format!("\u{8bbe}\u{8ba1}\u{6587}\u{6863}.md");
    let file_path = work_dir.join(&file_name);
    let file_content = "\u{4e2d}\u{6587}\u{5185}\u{5bb9}";
    std::fs::write(&file_path, file_content).expect("unicode file");

    let config = ShellHostConfig::new("line-unicode-path-test", &work_dir);
    let input = std::io::Cursor::new(format!("cat {}\necho after-cat\n", shell_arg(&file_path)));
    let mut rendered = Vec::new();
    let output =
        run_line_interactive_bash(&config, input, &mut rendered).expect("line interactive host");

    let rendered_text = String::from_utf8_lossy(&output.rendered_output);
    assert!(rendered_text.contains(file_content), "{rendered_text}");
    assert!(rendered_text.contains("after-cat"), "{rendered_text}");

    let replayed_events = read_shell_events(&output.shell.journal_path).expect("journal events");
    assert!(!replayed_events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("natural_language")
    }));

    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("cat ") && block.exit_code == 0));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-cat") && block.exit_code == 0));
}

#[test]
fn raw_relay_host_runs_top_and_keeps_shell_usable() {
    if Command::new("bash").arg("--version").output().is_err()
        || Command::new("top").arg("-h").output().is_err()
    {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-top-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-top-test", &work_dir);
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::line("top -l 1 2>/dev/null || top -bn1 2>/dev/null || echo top-skipped"),
            RawRelayAction::wait(Duration::from_millis(300)),
            RawRelayAction::write(b"q".to_vec()),
            RawRelayAction::wait(Duration::from_millis(100)),
            RawRelayAction::line("echo after-top"),
        ],
        &mut rendered,
    )
    .expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("after-top"), "{rendered_text}");
    assert_no_osc_marker(&rendered);
}

#[test]
fn raw_relay_host_runs_batchmode_ssh_without_swallowing_shell() {
    if Command::new("bash").arg("--version").output().is_err()
        || Command::new("ssh").arg("-V").output().is_err()
    {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-ssh-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-ssh-test", &work_dir);
    let ssh_command = "ssh -o BatchMode=yes -o ConnectTimeout=1 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null 127.0.0.1 true";
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::line(ssh_command),
            RawRelayAction::wait(Duration::from_millis(1500)),
            RawRelayAction::line("echo after-ssh"),
        ],
        &mut rendered,
    )
    .expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("after-ssh"), "{rendered_text}");
    assert_no_osc_marker(&rendered);

    let ledger = ledger_from_output(&output);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("ssh -o BatchMode=yes")));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-ssh") && block.exit_code == 0));
}

#[test]
#[ignore] // slow: creates fake sudo binary with timeout
    fn raw_relay_host_shows_isolated_sudo_prompt_and_keeps_shell_usable() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::path::PathBuf::from("/tmp").join(format!(
        "cosh-shell-raw-sudo-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let fake_bin_dir = work_dir.join("fake-bin");
    std::fs::create_dir_all(&fake_bin_dir).expect("fake bin dir");
    let fake_sudo = fake_bin_dir.join("sudo");
    std::fs::write(
        &fake_sudo,
        "#!/bin/sh\n\
         prompt='[sudo] password for cosh: '\n\
         while [ \"$#\" -gt 0 ]; do\n\
           case \"$1\" in\n\
             -p) shift; prompt=\"$1\" ;;\n\
           esac\n\
           shift || true\n\
         done\n\
         printf '%s' \"$prompt\" >&2\n\
         IFS= read -r _password\n",
    )
    .expect("fake sudo script");
    make_executable(&fake_sudo);

    let config = ShellHostConfig::new("raw-sudo-test", &work_dir);
    let command = format!(
        "PATH={}:$PATH sudo -p '[sudo] password for cosh: ' true",
        shell_arg(&fake_bin_dir)
    );
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash_with_actions(
        &config,
        vec![
            RawRelayAction::line(command),
            RawRelayAction::wait(Duration::from_millis(600)),
            RawRelayAction::write(vec![0x03]),
            RawRelayAction::line("echo after-sudo"),
        ],
        &mut rendered,
    )
    .expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("sudo"), "{rendered_text}");
    assert!(
        rendered_text.contains("password for cosh:"),
        "{rendered_text}"
    );
    assert!(rendered_text.contains("after-sudo"), "{rendered_text}");
    assert_no_osc_marker(&rendered);

    let ledger = ledger_from_output(&output);
    let sudo_block = ledger
        .blocks
        .iter()
        .find(|block| block.command.contains("sudo -p"))
        .expect("sudo command block");
    assert_ne!(sudo_block.exit_code, 0);
    let output_ref = sudo_block
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let output_ref_text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(
        output_ref_text.contains("[sudo] password for cosh:"),
        "{output_ref_text}"
    );
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-sudo") && block.exit_code == 0));
}

#[test]
fn raw_relay_host_intercepts_natural_language_via_bash_hook() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-hook-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-hook-test", &work_dir);
    let input = std::io::Cursor::new(
        "please analyze last failure\n\
         \u{8bf7}\u{5e2e}\u{6211}\u{5206}\u{6790}\n\
         missing-cosh-test-command\n\
         exit\n",
    );
    let mut rendered = Vec::new();
    let output = run_raw_relay_bash(&config, input, &mut rendered).expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(!rendered_text.contains("intercepted  natural_language"));
    assert!(!rendered
        .windows(b"\x1b]1337;COSH;".len())
        .any(|window| window == b"\x1b]1337;COSH;"));

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    assert!(replayed_events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("please analyze last failure")
            && event.component.as_deref() == Some("natural_language")
    }));
    assert!(replayed_events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.input.as_deref() == Some("\u{8bf7}\u{5e2e}\u{6211}\u{5206}\u{6790}")
            && event.component.as_deref() == Some("natural_language")
    }));

    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(!ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("please analyze last failure")));
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("missing-cosh-test-command") && block.exit_code != 0));
}

#[test]
fn raw_relay_host_can_render_inline_guidance_before_shell_exit() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-inline-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let config = ShellHostConfig::new("raw-inline-test", &work_dir);
    let input = std::io::Cursor::new(
        "/explain last error\n\
         ls /path/that/does/not/exist\n\
         echo after-inline\n\
         exit\n",
    );
    let mut rendered = Vec::new();
    let mut handled_blocks = HashSet::new();
    let output =
        run_raw_relay_bash_with_observer(&config, input, &mut rendered, |events, output| {
            let ledger = build_command_blocks(events);
            for block in ledger.blocks.iter().filter(|block| block.exit_code != 0) {
                if handled_blocks.insert(block.id.clone()) {
                    writeln!(output, "\n[inline] failed: {}", block.command)?;
                }
            }
            Ok(())
        })
        .expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    let inline_pos = rendered_text
        .find("[inline] failed")
        .expect("inline guidance");
    let after_pos = rendered_text
        .rfind("after-inline")
        .expect("continued shell command");
    assert!(inline_pos < after_pos, "{rendered_text}");

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-inline") && block.exit_code == 0));
}

#[test]
fn raw_relay_inline_governance_blocks_agent_execution_events() {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let work_dir = std::env::temp_dir().join(format!(
        "cosh-shell-raw-governance-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let side_effect_path = work_dir.join("agent-should-not-create-this");
    let config = ShellHostConfig::new("raw-governance-test", &work_dir);
    let input = std::io::Cursor::new(
        "/explain last error\n\
         ls /path/that/does/not/exist\n\
         echo after-governance\n\
         exit\n",
    );
    let mut rendered = Vec::new();
    let mut handled_blocks = HashSet::new();
    let side_effect_command = format!("touch {}", side_effect_path.display());
    let output =
        run_raw_relay_bash_with_observer(&config, input, &mut rendered, |events, output| {
            let ledger = build_command_blocks(events);
            for block in ledger.blocks.iter().filter(|block| block.exit_code != 0) {
                if !handled_blocks.insert(block.id.clone()) {
                    continue;
                }

                let agent_events = vec![
                    AgentEvent::ToolCall {
                        run_id: "run-1".to_string(),
                        name: "shell".to_string(),
                        input: side_effect_command.clone(),
                    },
                    AgentEvent::Action {
                        run_id: "run-1".to_string(),
                        command: side_effect_command.clone(),
                    },
                    AgentEvent::Recommendation {
                        run_id: "run-1".to_string(),
                        summary: "Try an unsafe auto fix".to_string(),
                        commands: vec![side_effect_command.clone()],
                        auto_execute: true,
                    },
                ];
                let governed = govern_agent_events(&agent_events, &Policy::default());
                assert!(governed.events.iter().all(|event| !event.auto_execute));
                assert!(governed
                    .events
                    .iter()
                    .any(|event| event.decision == GovernanceDecision::Rejected));
                assert!(governed
                    .events
                    .iter()
                    .any(|event| event.decision == GovernanceDecision::Degraded));

                writeln!(output)?;
                for event in governed.events {
                    writeln!(output, "{}", event.display_text)?;
                }
            }
            Ok(())
        })
        .expect("raw relay host");

    let rendered_text = String::from_utf8_lossy(&rendered);
    assert!(rendered_text.contains("Approval required: Bash command"));
    assert!(rendered_text.contains("Approval required: Shell command"));
    assert!(rendered_text.contains("Blocked: user approval required"));
    assert!(!rendered_text.contains("Decision: blocked by recommend-only governance"));
    assert!(rendered_text.contains("Try an unsafe auto fix"));
    assert!(rendered_text.contains("after-governance"));
    assert!(!side_effect_path.exists());

    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger
        .blocks
        .iter()
        .any(|block| block.command.contains("echo after-governance") && block.exit_code == 0));
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .expect("tool metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("tool permissions");
}

struct DelayedInput {
    chunks: Vec<(Vec<u8>, Duration)>,
    index: usize,
}

impl DelayedInput {
    fn new(chunks: Vec<(Vec<u8>, Duration)>) -> Self {
        Self { chunks, index: 0 }
    }
}

impl Read for DelayedInput {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some((chunk, delay)) = self.chunks.get(self.index) else {
            return Ok(0);
        };

        std::thread::sleep(*delay);
        let len = chunk.len().min(buf.len());
        buf[..len].copy_from_slice(&chunk[..len]);
        self.index += 1;
        Ok(len)
    }
}
