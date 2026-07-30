use super::*;
use crate::raw_input::UserPtyInputGeneration;

const TEST_MARKER_TOKEN: &str = "test-marker-token";

fn parser_for_test(name: &str) -> OscParser {
    let dir = std::env::temp_dir().join(format!("cosh-raw-relay-{name}"));
    OscParser::new(name.to_string(), dir, TEST_MARKER_TOKEN.to_string())
}

fn tracker_for_test() -> (UserPtyInputGeneration, PromptReplayTracker) {
    let generation = UserPtyInputGeneration::default();
    let tracker = PromptReplayTracker::new(generation.clone());
    (generation, tracker)
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
fn capture_ack_generation_expires_at_terminal_event() {
    let mut parser = parser_for_test("capture-ack-lifecycle");
    parser.push_capture_event(
        crate::types::ShellCaptureLifecycle::Submitted,
        7,
        Some("question"),
        Some("question-1"),
    );
    assert_eq!(
        latest_capture_submission_generation(&parser.events),
        Some(7)
    );

    parser.push_capture_event(crate::types::ShellCaptureLifecycle::Drained, 7, None, None);
    assert_eq!(latest_capture_submission_generation(&parser.events), None);
}

#[test]
fn handoff_prompt_restore_strips_duplicate_prompt_echo() {
    let mut parser = parser_for_test("handoff-prompt-restore");
    feed_shell_ready(&mut parser);
    parser.feed(b"bash-4.4$ ").expect("feed prompt");
    let mut display_start = parser.display.len();
    let (_generation, mut prompt_replay) = tracker_for_test();
    let mut output = Vec::new();

    restore_prompt_display_before_handoff(
        &parser,
        &mut output,
        &mut display_start,
        &mut prompt_replay,
    )
    .expect("restore prompt");

    assert_eq!(String::from_utf8_lossy(&output), "bash-4.4$ ");
    assert!(prompt_replay.is_armed());

    parser
        .feed(b"bash-4.4$ echo ok\r\n")
        .expect("feed echoed handoff");
    write_pending_display(&parser, &mut output, &mut display_start, &mut prompt_replay)
        .expect("write echoed handoff");

    assert_eq!(String::from_utf8_lossy(&output), "bash-4.4$ echo ok\r\n");
    assert!(!prompt_replay.is_armed());
}

#[test]
fn user_pty_input_expires_armed_prompt_replay_before_echo_is_parsed() {
    let mut parser = parser_for_test("user-input-expires-replay");
    feed_shell_ready(&mut parser);
    parser.feed(b"prompt> \x1b[?2004h").expect("feed prompt");
    let mut display_start = parser.display.len();
    let (generation, mut prompt_replay) = tracker_for_test();
    let mut output = Vec::new();

    restore_prompt_display_before_handoff(
        &parser,
        &mut output,
        &mut display_start,
        &mut prompt_replay,
    )
    .expect("restore prompt");
    assert!(prompt_replay.is_armed());

    // A real empty Enter: the relay bumps the generation before writing to
    // the PTY, so its echo (bracketed-paste toggle + CRLF + fresh prompt)
    // must not be deduplicated as the replayed prompt.
    generation.bump();
    parser
        .feed(b"\x1b[?2004l\r\r\n")
        .expect("feed empty enter accept");
    write_pending_display(&parser, &mut output, &mut display_start, &mut prompt_replay)
        .expect("write empty enter accept");
    parser
        .feed(b"prompt> \x1b[?2004h")
        .expect("feed fresh prompt");
    write_pending_display(&parser, &mut output, &mut display_start, &mut prompt_replay)
        .expect("write fresh prompt");

    assert_eq!(
        String::from_utf8_lossy(&output),
        "prompt> \x1b[?2004h\x1b[?2004l\r\r\nprompt> \x1b[?2004h"
    );
    assert!(!prompt_replay.is_armed());
}

#[test]
fn relay_write_event_expires_armed_prompt_replay() {
    let (generation, mut prompt_replay) = tracker_for_test();

    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());

    // The write event for a user keystroke travels through the event
    // channel ahead of the echo bytes it triggers.
    prompt_replay.observe_user_write(generation.bump(), 1);

    assert!(!prompt_replay.is_armed());
}

#[test]
fn prompt_restore_refuses_to_arm_while_user_input_response_is_unparsed() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // An empty Enter reached the PTY inside the Delay window; no prompt
    // boundary has confirmed its response was parsed yet.
    generation.bump();

    prompt_replay.arm_for_replay(b"prompt> ");

    assert!(!prompt_replay.is_armed());
}

#[test]
fn handoff_refuses_to_arm_over_pending_user_input() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // A user write raced in after the output loop's last event drain and
    // before the handoff prompt restore runs: bump -> arm -> user echo.
    generation.bump();
    prompt_replay.arm_for_replay(b"prompt> \x1b[?2004h");
    assert!(!prompt_replay.is_armed());

    // The user's accept-line echo must pass through untouched.
    assert_eq!(
        prompt_replay.strip(b"\x1b[?2004l\r\r\n"),
        b"\x1b[?2004l\r\r\n"
    );
    assert_eq!(
        prompt_replay.strip(b"prompt> \x1b[?2004h"),
        b"prompt> \x1b[?2004h"
    );
}

#[test]
fn prompt_restore_arms_again_after_the_response_reaches_a_prompt_boundary() {
    let (generation, mut prompt_replay) = tracker_for_test();

    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());

    // precmd boundary: the Enter response is fully parsed.
    prompt_replay.observe_prompt_boundary();
    prompt_replay.arm_for_replay(b"prompt> ");

    assert!(prompt_replay.is_armed());
}

#[test]
fn typeahead_submission_blocks_arm_until_its_own_prompt_boundary() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // A command line, then an empty Enter typed ahead while it runs; both
    // write events are drained before the command's precmd arrives.
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_user_write(generation.bump(), 1);

    // The command's precmd only accounts for the command's own submission,
    // so a command insight or handoff must not arm over the queued Enter.
    prompt_replay.observe_prompt_boundary();
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());

    // The empty Enter's own boundary settles the ledger.
    prompt_replay.observe_prompt_boundary();
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());
}

#[test]
fn several_enters_in_one_write_block_arm_until_all_boundaries_arrive() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // One relay write carrying two accept-line keys (e.g. "\r\r").
    prompt_replay.observe_user_write(generation.bump(), 2);

    prompt_replay.observe_prompt_boundary();
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());

    prompt_replay.observe_prompt_boundary();
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());
}

#[test]
fn boundary_without_a_matching_submission_is_ignored() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // A Ctrl-C style prompt repaint yields a boundary with no submission.
    prompt_replay.observe_prompt_boundary();

    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());

    prompt_replay.observe_prompt_boundary();
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());
}

#[test]
fn intercepted_submission_does_not_block_arming_over_its_repaint() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // A recalled slash line is submitted and killed by the DEBUG trap; its
    // precmd has not arrived when the panel's RestorePrompt fires, but its
    // only remaining response is the repaint the arm exists to strip.
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_intercept_cut();

    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());
}

#[test]
fn typeahead_enter_still_blocks_arming_alongside_an_intercept() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // Recalled slash + empty Enter in one write: only the slash line was
    // intercepted; the Enter's response is still pending.
    prompt_replay.observe_user_write(generation.bump(), 2);
    prompt_replay.observe_intercept_cut();

    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());
}

#[test]
fn uncounted_custom_accept_line_intercept_never_swallows_typeahead_enter() {
    let (generation, mut prompt_replay) = tracker_for_test();

    // `.inputrc` binds C-x to accept-line: the intercepted slash line's
    // submission is invisible to the byte counter, so the typeahead Enter in
    // the same write is the only counted submit and the intercept cut lets
    // the panel restore arm over it.
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_intercept_cut();
    prompt_replay.arm_for_replay(b"prompt> \x1b[?2004h");
    assert!(prompt_replay.is_armed());

    // Bash's post-intercept repaint opens with the prompt and is deduped.
    assert_eq!(prompt_replay.strip(b"prompt> \x1b[?2004h"), b"");
    // The queued Enter's response never opens with the prompt bytes, so the
    // mis-attributed arm cannot swallow it: toggle, CRLF, and the fresh
    // prompt all reach the terminal verbatim.
    assert_eq!(
        prompt_replay.strip(b"\x1b[?2004l\r\r\n"),
        b"\x1b[?2004l\r\r\n"
    );
    assert_eq!(
        prompt_replay.strip(b"prompt> \x1b[?2004h"),
        b"prompt> \x1b[?2004h"
    );
}

#[test]
fn foreground_consumed_submission_is_written_off_at_an_idle_prompt() {
    let (generation, mut prompt_replay) = tracker_for_test();
    prompt_replay.set_idle_reconcile_window(Duration::from_millis(0));

    // `read value` (one submit) plus "hello\r" consumed by `read` itself:
    // only the command's precmd arrives, leaving a permanent +1 balance.
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_prompt_boundary();

    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());

    // Shell idles at the painted prompt: the orphaned submission is
    // reconciled and replay dedup recovers for the rest of the session.
    prompt_replay.reconcile_idle_at_prompt(false, true, None);
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());
}

#[test]
fn idle_reconcile_is_refused_while_a_command_is_running() {
    let (generation, mut prompt_replay) = tracker_for_test();
    prompt_replay.set_idle_reconcile_window(Duration::from_millis(0));

    // Command submitted, then a typeahead Enter while it runs.
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_prompt_boundary();

    // A long-running command keeps the PTY silent, but the typeahead Enter
    // is still queued: it must not be written off.
    prompt_replay.reconcile_idle_at_prompt(true, true, None);
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());
}

#[test]
fn idle_reconcile_is_refused_until_the_prompt_is_painted() {
    let (generation, mut prompt_replay) = tracker_for_test();
    prompt_replay.set_idle_reconcile_window(Duration::from_millis(0));

    // Command plus a typeahead Enter; the command's precmd marker arrived
    // but the user's slow PROMPT_COMMAND still runs, so no prompt bytes are
    // painted and readline has not consumed the queued Enter yet.
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_user_write(generation.bump(), 1);
    prompt_replay.observe_prompt_boundary();

    // However long the silence lasts, the ledger must survive until the
    // shell actually paints a prompt: a precmd marker alone is not consent.
    prompt_replay.reconcile_idle_at_prompt(false, false, None);
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());

    // Once the prompt is painted, readline echoes the queued Enter and its
    // own boundary settles the ledger through the normal path.
    prompt_replay.observe_prompt_boundary();
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());
}

#[test]
fn idle_reconcile_waits_for_pty_and_relay_silence() {
    let (generation, mut prompt_replay) = tracker_for_test();

    prompt_replay.observe_user_write(generation.bump(), 1);

    // A write just drained: the reconcile window has not elapsed, so the
    // pending submission survives (its echo may still be in flight).
    prompt_replay.reconcile_idle_at_prompt(false, true, Some(Instant::now()));
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());

    prompt_replay.set_idle_reconcile_window(Duration::from_millis(0));
    prompt_replay.reconcile_idle_at_prompt(false, true, None);
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(prompt_replay.is_armed());
}

#[test]
fn idle_reconcile_is_refused_while_a_write_is_undrained() {
    let (generation, mut prompt_replay) = tracker_for_test();
    prompt_replay.set_idle_reconcile_window(Duration::from_millis(0));

    prompt_replay.observe_user_write(generation.bump(), 1);
    // A second write bumped the shared generation but its event has not
    // been drained yet: the ledger is not settled.
    generation.bump();

    prompt_replay.reconcile_idle_at_prompt(false, true, None);
    prompt_replay.arm_for_replay(b"prompt> ");
    assert!(!prompt_replay.is_armed());
}

#[test]
fn prompt_restore_arms_when_no_user_input_followed_the_intercept() {
    let (_generation, mut prompt_replay) = tracker_for_test();

    prompt_replay.arm_for_replay(b"prompt> ");

    assert!(prompt_replay.is_armed());
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
    let (_generation, mut prompt_replay) = tracker_for_test();
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
        &mut prompt_replay,
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
        &mut prompt_replay,
        &input_mode,
    )
    .expect("write prompt fragment");

    assert!(
        output.ends_with(b"\x1b[s\x1b[2m objdump\x1b[0m\x1b[u"),
        "{}",
        String::from_utf8_lossy(&output)
    );
}
