use super::*;

fn snap(echo_off: bool, icanon: bool, blocked: bool) -> InteractiveSnapshot {
    InteractiveSnapshot {
        echo_off,
        icanon,
        fg_is_foreign: true,
        blocked_tty_read: blocked,
        fg_comm: Some("probe".to_string()),
    }
}

#[test]
fn decision_table_rows_match_probe_matrix_anchors() {
    // Row 1: read -s / getpass / sudo fingerprint.
    assert_eq!(
        classify_interactive_state(&snap(true, true, true), false, None),
        Some(InteractiveHintKind::Password)
    );
    // Row 2: alt-screen wins for any snapshot (vi, less with smcup).
    assert!(matches!(
        classify_interactive_state(&snap(true, false, false), true, None),
        Some(InteractiveHintKind::Fullscreen { .. })
    ));
    // Row 3: less/more between keys.
    assert!(matches!(
        classify_interactive_state(&snap(true, false, true), false, None),
        Some(InteractiveHintKind::Pager { .. })
    ));
    // Row 4: top / python repl (select loop, no blocking read).
    assert!(matches!(
        classify_interactive_state(&snap(true, false, false), false, None),
        Some(InteractiveHintKind::RawInteractive { .. })
    ));
    // Row 5: rm -i / cat.
    assert_eq!(
        classify_interactive_state(&snap(false, true, true), false, None),
        Some(InteractiveHintKind::StdinWait)
    );
    // Row 6: sleep (cooked, no blocked read) and busy loop.
    assert_eq!(
        classify_interactive_state(&snap(false, true, false), false, None),
        None
    );
    assert_eq!(
        classify_interactive_state(&snap(false, false, false), false, None),
        None
    );
}

#[test]
fn sampler_error_snapshot_classifies_to_none() {
    assert_eq!(
        classify_interactive_state(&InteractiveSnapshot::default(), false, None),
        None
    );
}

#[test]
fn missing_comm_keeps_classification() {
    let mut snapshot = snap(true, false, true);
    snapshot.fg_comm = None;
    assert_eq!(
        classify_interactive_state(&snapshot, false, None),
        Some(InteractiveHintKind::Pager { comm: None })
    );
}

#[test]
fn static_prior_never_gates_classification() {
    assert_eq!(
        classify_interactive_state(&snap(false, true, false), false, Some("needs a tty")),
        None
    );
    assert_eq!(
        classify_interactive_state(&snap(true, true, false), false, Some("needs a tty")),
        Some(InteractiveHintKind::Password)
    );
}

#[test]
fn throttle_requires_quiet_window_and_interval() {
    let mut throttle = SentinelThrottle::new();
    let start = Instant::now();
    assert!(!throttle.should_sample(start));
    let quiet = start + SENTINEL_QUIET + Duration::from_millis(10);
    assert!(throttle.should_sample(quiet));
    assert!(!throttle.should_sample(quiet + Duration::from_millis(100)));
    assert!(throttle.should_sample(quiet + SENTINEL_SAMPLE_INTERVAL + Duration::from_millis(10)));
    throttle.note_output();
    assert!(!throttle.should_sample(Instant::now()));
}

#[test]
fn timeout_eligibility_requires_blocked_read_evidence() {
    // Eligible: blocked-tty-read kinds (#2161 harm classes).
    assert!(timeout_eligible(&InteractiveHintKind::Password));
    assert!(timeout_eligible(&InteractiveHintKind::StdinWait));
    assert!(timeout_eligible(&InteractiveHintKind::Pager { comm: None }));
    // Exempt: alt-screen TUIs (D10) and evidence-free select loops.
    assert!(!timeout_eligible(&InteractiveHintKind::Fullscreen {
        comm: None
    }));
    assert!(!timeout_eligible(&InteractiveHintKind::RawInteractive {
        comm: None
    }));
}

#[test]
fn hint_card_covers_presentation_contract() {
    let i18n = crate::i18n::I18n::new(crate::config::Language::EnUs);
    // Fullscreen: in-screen insert exempt (D10) => nothing rendered.
    assert_eq!(
        hint_card(
            &InteractiveHintKind::Fullscreen { comm: None },
            "anything",
            &i18n,
            120,
            80
        ),
        None
    );
    // Prompt tail present: card shows the tail and redraws it after.
    let card = hint_card(
        &InteractiveHintKind::StdinWait,
        "Confirm? (y/n):",
        &i18n,
        120,
        80,
    )
    .expect("card rendered");
    assert!(card.contains("Command is waiting for input"), "{card}");
    assert!(
        card.matches("Confirm? (y/n):").count() >= 2,
        "tail must appear in the card body and in the redraw: {card}"
    );
    assert!(card.contains("Auto-interrupts after 120s"), "{card}");
    assert!(card.trim_end().ends_with("Confirm? (y/n):"), "{card}");
    // Timeout disabled: no forecast line.
    let card = hint_card(
        &InteractiveHintKind::StdinWait,
        "Confirm? (y/n):",
        &i18n,
        0,
        80,
    )
    .expect("card rendered");
    assert!(!card.contains("Auto-interrupts"), "{card}");
    // Empty tail (e.g. read -s without prompt text): kind body only,
    // no redraw suffix.
    let card =
        hint_card(&InteractiveHintKind::Password, "", &i18n, 120, 80).expect("card rendered");
    assert!(card.contains("password/hidden input"), "{card}");
    assert!(
        card.trim_end().ends_with('\u{1b}') || card.ends_with("\r\n"),
        "{card}"
    );
}

#[test]
fn prompt_tail_extraction_strips_escapes_and_controls() {
    let display = b"line one\n\x1b[1;32mConfirm?\x1b[0m (y/n): \x07";
    assert_eq!(prompt_tail_from_display(display), "Confirm? (y/n): ");
    assert_eq!(prompt_tail_from_display(b""), "");
    let osc = b"step\n\x1b]0;title\x07Grant it? ";
    assert_eq!(prompt_tail_from_display(osc), "Grant it? ");
}

#[test]
fn strip_escape_sequences_covers_string_family_and_nf_variants() {
    // DCS / APC payloads terminated by ST must not leak (#2168 review).
    assert_eq!(
        strip_escape_sequences("\x1bPpayload\x1b\\Continue? "),
        "Continue? "
    );
    assert_eq!(
        strip_escape_sequences("\x1b_meta\x07Proceed? "),
        "Proceed? "
    );
    // Charset selection `ESC ( B` drops the final byte too.
    assert_eq!(strip_escape_sequences("\x1b(BAnswer: "), "Answer: ");
    // Two-char escapes and an orphan trailing ESC stay silent.
    assert_eq!(strip_escape_sequences("\x1b=ok\x1b"), "ok");
    // Unterminated string sequences swallow the tail (fail-quiet:
    // an empty tail suppresses the redraw, never shows raw bytes).
    assert_eq!(strip_escape_sequences("\x1b]0;title"), "");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_probe_requires_tty_evidence_for_blocked_reads() {
    // read(2) on both supported arches parses the fd argument.
    assert_eq!(linux::blocked_read_fd("0 0x0 0x7ffd 0x2000"), Some(0));
    assert_eq!(linux::blocked_read_fd("63 0x3 0x7ffd 0x2000"), Some(3));
    // Non-read syscalls and running processes never count.
    assert_eq!(linux::blocked_read_fd("7 0x0"), None);
    assert_eq!(linux::blocked_read_fd("running"), None);
    // Only terminal device targets qualify as tty evidence.
    assert!(linux::tty_path("/dev/pts/3"));
    assert!(linux::tty_path("/dev/tty1"));
    assert!(linux::tty_path("/dev/console"));
    assert!(!linux::tty_path("pipe:[12345]"));
    assert!(!linux::tty_path("/tmp/file"));
    assert!(!linux::tty_path("socket:[999]"));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_fallback_scan_keeps_full_probe_budget() {
    // #2176 review P2: a missed leader shortcut must not consume
    // fallback slots — a blocked reader sitting in the *last* budget
    // slot (the 32nd non-leader member in /proc order) is still probed
    // and still produces the verdict, exactly as the pre-shortcut scan
    // did.
    let budget = linux::MAX_PGRP_PROBES_FOR_TEST;
    let blocked_pid = budget as i32; // last member inside the budget
    let mut probes = Vec::new();
    let hit = linux::scan_members(1..=blocked_pid, |pid| {
        probes.push(pid);
        pid == blocked_pid
    });
    assert!(hit, "evidence in the last budget slot must be found");
    assert_eq!(probes.len(), budget, "every budget slot must be probed");

    // The bound itself stays fail-quiet: a member beyond the budget is
    // never probed, and the scan reports no evidence.
    let mut probed_beyond = false;
    let over_budget = blocked_pid + 1;
    let hit = linux::scan_members(1..=over_budget, |pid| {
        probed_beyond |= pid == over_budget;
        pid == over_budget
    });
    assert!(!hit);
    assert!(!probed_beyond, "budget bound must cap the scan");

    // Early evidence short-circuits the walk.
    let mut count = 0;
    assert!(linux::scan_members(1..=blocked_pid, |pid| {
        count += 1;
        pid == 3
    }));
    assert_eq!(count, 3);
}

#[test]
fn input_wait_status_marks_clears_and_restarts() {
    let status = InputWaitStatus::default();
    assert_eq!(status.waiting_for(), None);
    status.mark_waiting();
    let first = status.waiting_for().expect("episode running");
    // A second mark keeps the original episode start.
    status.backdate_for_test(Duration::from_secs(30));
    status.mark_waiting();
    let aged = status.waiting_for().expect("episode still running");
    assert!(aged >= Duration::from_secs(30), "{aged:?}");
    assert!(aged >= first);
    // Clearing resets; a new mark restarts from zero.
    status.clear();
    assert_eq!(status.waiting_for(), None);
    status.mark_waiting();
    let restarted = status.waiting_for().expect("restarted episode");
    assert!(restarted < Duration::from_secs(30), "{restarted:?}");
}

#[test]
fn input_wait_clock_is_jump_defensive() {
    // Drive wall-clock jump scenarios through the production call chain
    // (`mark_waiting` -> `waiting_for`) on a pinned episode clock
    // (#2176 review P2): the consumer-visible duration must reflect
    // exactly the episode-clock delta, never a jump artifact.
    let base = 1u64 << 32;
    let status = InputWaitStatus::default();

    // Forward jump shape: a timeout may only fire after the configured
    // wait truly elapsed on the episode clock. With the former
    // `SystemTime` chain a wall jump inflated this reading and fired a
    // false SIGINT seconds into a legitimate wait.
    status.set_test_clock_ms(base);
    status.mark_waiting();
    assert_eq!(status.waiting_for(), Some(Duration::ZERO));
    status.set_test_clock_ms(base + 120_000);
    assert_eq!(status.waiting_for(), Some(Duration::from_millis(120_000)));

    // Backward jump artifact (start stamp ahead of "now"): re-mark at a
    // late reading, then pull the clock back — the duration clamps to
    // zero instead of underflowing into a giant elapsed value; the
    // episode keeps running and resumes counting past the stamp.
    status.clear();
    status.set_test_clock_ms(base + 120_000);
    status.mark_waiting();
    status.set_test_clock_ms(base + 115_000);
    assert_eq!(status.waiting_for(), Some(Duration::ZERO));
    status.set_test_clock_ms(base + 180_000);
    assert_eq!(status.waiting_for(), Some(Duration::from_millis(60_000)));

    // A producer clear/re-mark under the pinned clock restarts the
    // episode from the current reading, not from any stale stamp.
    status.clear();
    status.mark_waiting();
    assert_eq!(status.waiting_for(), Some(Duration::ZERO));

    // Clock-source discriminator: the real readings are anchored to a
    // process-local `Instant` epoch, so `now_ms - offset` is process
    // uptime — orders of magnitude below UNIX-epoch milliseconds
    // (~1.7e12 in 2026). Reverting `now_ms` to a `SystemTime`-derived
    // value trips this bound immediately.
    let uptime_ms = InputWaitStatus::now_ms() - base;
    assert!(
        uptime_ms < 1_000_000_000_000, // ~31.7 years of process uptime
        "now_ms looks wall-clock derived: {uptime_ms}ms past offset"
    );
    // Monotonic readings are strictly non-decreasing across calls, and
    // the offset keeps every reading past the "not waiting" sentinel
    // with backdating headroom.
    let a = InputWaitStatus::now_ms();
    let b = InputWaitStatus::now_ms();
    assert!(b >= a, "monotonic clock went backwards: {a} -> {b}");
    assert!(a >= base);
}
