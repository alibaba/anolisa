use super::*;

const TEST_MARKER_TOKEN: &str = "test-marker-token";

fn parser_for_test(name: &str) -> OscParser {
    let dir = std::env::temp_dir().join(format!("cosh-raw-relay-{name}"));
    OscParser::new(name.to_string(), dir, TEST_MARKER_TOKEN.to_string())
}

fn feed_shell_ready(parser: &mut OscParser) {
    let mut marker = Vec::new();
    marker.extend_from_slice(b"\x1b]1337;COSH;");
    marker.extend_from_slice(
        br#"{"event":"precmd","token":"test-marker-token","status":0,"cwd":"/tmp"}"#,
    );
    marker.push(b'\x07');
    parser.feed(&marker).expect("feed precmd");
}

#[test]
fn handoff_prompt_restore_strips_duplicate_prompt_echo() {
    let mut parser = parser_for_test("handoff-prompt-restore");
    feed_shell_ready(&mut parser);
    parser.feed(b"bash-4.4$ ").expect("feed prompt");
    let mut display_start = parser.display.len();
    let mut replayed_prompt_prefix = None;
    let mut output = Vec::new();

    restore_prompt_display_before_handoff(
        &parser,
        &mut output,
        &mut display_start,
        &mut replayed_prompt_prefix,
    )
    .expect("restore prompt");

    assert_eq!(String::from_utf8_lossy(&output), "bash-4.4$ ");
    assert_eq!(replayed_prompt_prefix.as_deref(), Some(&b"bash-4.4$ "[..]));

    parser
        .feed(b"bash-4.4$ echo ok\r\n")
        .expect("feed echoed handoff");
    write_pending_display(
        &parser,
        &mut output,
        &mut display_start,
        &mut replayed_prompt_prefix,
    )
    .expect("write echoed handoff");

    assert_eq!(String::from_utf8_lossy(&output), "bash-4.4$ echo ok\r\n");
    assert!(replayed_prompt_prefix.is_none());
}

#[test]
fn prompt_restore_waits_through_passive_observer_cycles() {
    let restore = RawObserverAction::RestorePrompt {
        ghost_text: Some("analyze failure".to_string()),
        ghost_route: Default::default(),
    };
    let mut pending = None;
    remember_pending_prompt_restore(&restore, &mut pending);

    assert_eq!(
        merge_pending_prompt_restore(RawObserverAction::Continue, &mut pending),
        restore
    );
    assert!(pending.is_none());
}

#[test]
fn active_observer_action_supersedes_waiting_prompt_restore() {
    let restore = RawObserverAction::RestorePrompt {
        ghost_text: Some("analyze failure".to_string()),
        ghost_route: Default::default(),
    };
    let mut pending = Some(restore);

    let observed = RawObserverAction::HoldShellOutput;
    assert_eq!(
        merge_pending_prompt_restore(observed.clone(), &mut pending),
        observed
    );
    assert!(pending.is_none());
}

#[test]
fn foreground_passthrough_cancels_waiting_prompt_restore() {
    let restore = RawObserverAction::RestorePrompt {
        ghost_text: Some("analyze failure".to_string()),
        ghost_route: Default::default(),
    };
    let mut pending = Some(restore);

    assert_eq!(
        merge_pending_prompt_restore(RawObserverAction::RawPassthrough, &mut pending),
        RawObserverAction::RawPassthrough
    );
    assert!(pending.is_none());
}

#[test]
fn prompt_fragment_after_restore_keeps_ghost_last_on_screen() {
    let mut parser = parser_for_test("fragmented-prompt-ghost");
    feed_shell_ready(&mut parser);
    parser
        .feed(b"\x1b]0;root@host\x07")
        .expect("feed title fragment");
    let mut output = Vec::new();
    let mut display_start = 0;
    let mut replayed_prompt_prefix = None;
    let input_mode = Arc::new(Mutex::new(RawInputMode::Passthrough));
    let mut pending_terminal_restore = PendingTerminalRecovery::default();
    let mut null = File::open("/dev/null").expect("open null");

    let action = resolve_pty_emit(
        &mut null,
        1,
        -1,
        &mut parser,
        &mut output,
        &input_mode,
        RawObserverAction::RestorePrompt {
            ghost_text: Some("objdump".to_string()),
            ghost_route: Default::default(),
        },
        &mut display_start,
        &mut replayed_prompt_prefix,
        &mut pending_terminal_restore,
        Path::new("/tmp/cosh-test-recovery"),
        Path::new("/tmp/cosh-test-handoff"),
    )
    .expect("restore prompt");
    assert_eq!(action, RawObserverAction::Continue);

    parser
        .feed(b"\x1b[?2004h[root@host]# ")
        .expect("feed prompt fragment");
    write_pending_display_preserving_prompt_ghost(
        &parser,
        &mut output,
        &mut display_start,
        &mut replayed_prompt_prefix,
        &input_mode,
    )
    .expect("write prompt fragment");

    assert!(
        output.ends_with(b"\x1b[s\x1b[2m objdump\x1b[0m\x1b[u"),
        "{}",
        String::from_utf8_lossy(&output)
    );
}
