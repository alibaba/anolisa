use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::super::spawn::{finish_input_relay, relay_input_bytes, RawInputRelayState};
use super::super::{PromptGhostCandidate, RawRelayAction};
use super::*;

fn output_file(label: &str) -> (std::path::PathBuf, File) {
    let path = std::env::temp_dir().join(format!(
        "cosh-shell-prompt-ghost-{label}-{}",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .expect("test output file");
    (path, file)
}

fn selection_input_mode() -> Arc<Mutex<RawInputMode>> {
    let candidates = vec![
        PromptGhostCandidate {
            text: "inspect memory".to_string(),
            suggestion_id: "health-1".to_string(),
        },
        PromptGhostCandidate {
            text: "continue deployment".to_string(),
            suggestion_id: "personal-1".to_string(),
        },
    ];
    Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: candidates[0].text.clone(),
        route: PromptGhostRoute::AgentSelection {
            candidates,
            active: 0,
        },
    }))
}

fn expect_prompt_ghost_dismissal(receiver: &mpsc::Receiver<RawInputEvent>) {
    for _ in 0..2 {
        if receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("prompt ghost dismissal event")
            == RawInputEvent::PromptGhostDismissed
        {
            return;
        }
    }
    panic!("missing prompt ghost dismissal event");
}

struct ChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() {
            match self.receiver.recv() {
                Ok(bytes) => self.pending = bytes,
                Err(_) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.pending.len());
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Ok(count)
    }
}

struct SelectionRelay {
    path: std::path::PathBuf,
    master: File,
    input_tx: Option<mpsc::Sender<Vec<u8>>>,
    event_rx: mpsc::Receiver<RawInputEvent>,
    input_mode: Arc<Mutex<RawInputMode>>,
    relay: thread::JoinHandle<io::Result<()>>,
}

impl SelectionRelay {
    fn start(label: &str) -> Self {
        let (path, master) = output_file(label);
        let (input_tx, input_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let input_mode = selection_input_mode();
        let relay = super::super::spawn_raw_input_relay(
            ChannelReader {
                receiver: input_rx,
                pending: Vec::new(),
            },
            master.try_clone().expect("clone output file"),
            event_tx,
            InputClassifier::default(),
            input_mode.clone(),
        );
        Self {
            path,
            master,
            input_tx: Some(input_tx),
            event_rx,
            input_mode,
            relay,
        }
    }

    fn send(&self, bytes: &[u8]) {
        self.input_tx
            .as_ref()
            .expect("input sender")
            .send(bytes.to_vec())
            .expect("send input");
    }

    fn finish(mut self) -> (Vec<RawInputEvent>, Vec<u8>, RawInputMode) {
        self.input_tx.take();
        self.relay
            .join()
            .expect("relay thread")
            .expect("relay result");
        self.master.sync_all().expect("sync test output");
        let output = fs::read(&self.path).expect("read test output");
        fs::remove_file(&self.path).ok();
        let mode = self.input_mode.lock().expect("input mode").clone();
        (self.event_rx.try_iter().collect(), output, mode)
    }
}

#[test]
fn selection_bare_escape_times_out_without_waiting_for_another_key() {
    let (path, master) = output_file("selection-bare-escape");
    let (input_tx, input_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let candidates = vec![PromptGhostCandidate {
        text: "inspect memory".to_string(),
        suggestion_id: "health-1".to_string(),
    }];
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: candidates[0].text.clone(),
        route: PromptGhostRoute::AgentSelection {
            candidates,
            active: 0,
        },
    }));
    let relay = super::super::spawn_raw_input_relay(
        ChannelReader {
            receiver: input_rx,
            pending: Vec::new(),
        },
        master.try_clone().expect("clone output file"),
        event_tx,
        InputClassifier::default(),
        input_mode.clone(),
    );

    input_tx.send(b"\x1b".to_vec()).expect("send escape");
    expect_prompt_ghost_dismissal(&event_rx);
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));

    drop(input_tx);
    relay.join().expect("relay thread").expect("relay result");
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bexit\n");
    fs::remove_file(path).ok();
}

#[test]
fn selection_action_wait_flushes_escape_at_the_deadline() {
    let (path, master) = output_file("selection-action-wait");
    let (tx, rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "inspect memory".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![PromptGhostCandidate {
                text: "inspect memory".to_string(),
                suggestion_id: "health-1".to_string(),
            }],
            active: 0,
        },
    }));
    let relay = super::super::spawn_raw_action_relay(
        vec![
            RawRelayAction::write(b"\x1b"),
            RawRelayAction::wait(Duration::from_millis(500)),
        ],
        master.try_clone().expect("clone output file"),
        0,
        tx,
        InputClassifier::default(),
        input_mode.clone(),
    );

    expect_prompt_ghost_dismissal(&rx);
    assert!(!relay.is_finished());
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));

    relay.join().expect("relay thread").expect("relay result");
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bexit\n");
    fs::remove_file(path).ok();
}

#[test]
fn selection_shift_tab_cycles_when_arriving_in_one_chunk() {
    let relay = SelectionRelay::start("selection-shift-tab");
    relay.send(b"\x1b[Z");

    let (events, output, mode) = relay.finish();

    assert_eq!(output, b"exit\n");
    assert!(events.contains(&RawInputEvent::PromptGhostCycle {
        text: "continue deployment".to_string(),
    }));
    assert!(matches!(
        mode,
        RawInputMode::PromptGhost { text, .. } if text == "continue deployment"
    ));
}

#[test]
fn selection_shift_tab_cycles_when_arriving_in_three_chunks_within_window() {
    let (path, mut master) = output_file("selection-split-shift-tab");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    for (bytes, offset) in [
        (b"\x1b".as_slice(), 0),
        (b"[".as_slice(), 1),
        (b"Z".as_slice(), 2),
    ] {
        relay_input_bytes(
            bytes,
            received_at + Duration::from_millis(offset),
            &mut master,
            &tx,
            &classifier,
            &input_mode,
            &mut state,
        )
        .expect("relay split shift-tab");
    }

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. }))
            .count(),
        1
    );
    fs::remove_file(path).ok();
}

#[test]
fn selection_shift_tab_received_before_deadline_survives_a_delayed_relay() {
    let (path, mut master) = output_file("selection-delayed-shift-tab");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();

    relay_input_bytes(
        b"\x1b",
        Instant::now()
            .checked_sub(Duration::from_millis(100))
            .expect("recent timestamp"),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    let received_at = Instant::now()
        .checked_sub(Duration::from_millis(90))
        .expect("recent timestamp");
    relay_input_bytes(
        b"[Z",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("cycle delayed shift-tab");

    assert!(rx.try_iter().any(|event| matches!(
        event,
        RawInputEvent::PromptGhostCycle { text } if text == "continue deployment"
    )));
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    fs::remove_file(path).ok();
}

#[test]
fn selection_escape_with_nonmatching_follow_up_dismisses_and_forwards_all_bytes() {
    let relay = SelectionRelay::start("selection-escape-nonmatching");
    relay.send(b"\x1b");
    relay.send(b"x");

    let (events, output, mode) = relay.finish();

    assert_eq!(output, b"\x1bxexit\n");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. })));
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_partial_csi_times_out_and_forwards_all_bytes() {
    let relay = SelectionRelay::start("selection-partial-csi");
    relay.send(b"\x1b[");
    expect_prompt_ghost_dismissal(&relay.event_rx);

    let (_, output, mode) = relay.finish();
    assert_eq!(output, b"\x1b[exit\n");
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_pending_escape_at_eof_dismisses_and_forwards_escape() {
    let relay = SelectionRelay::start("selection-escape-eof");
    relay.send(b"\x1b");

    let (events, output, mode) = relay.finish();

    assert_eq!(output, b"\x1bexit\n");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_pending_escape_at_eof_after_route_change_is_not_dropped() {
    let (path, mut master) = output_file("selection-escape-route-eof");
    let (tx, rx) = mpsc::channel();
    let old_route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "old selection".to_string(),
            suggestion_id: "old-1".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "old selection".to_string(),
        route: old_route,
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    relay_input_bytes(
        b"\x1b",
        Instant::now(),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::PromptGhost {
        text: "new selection".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![PromptGhostCandidate {
                text: "new selection".to_string(),
                suggestion_id: "new-1".to_string(),
            }],
            active: 0,
        },
    };
    finish_input_relay(&mut master, &tx, &classifier, &input_mode, &mut state)
        .expect("finish relay");

    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bexit\n");
    assert!(rx
        .try_iter()
        .any(|event| event == RawInputEvent::PromptGhostDismissed));
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn selection_route_change_before_deadline_dismisses_then_forwards_shift_tab() {
    let (path, mut master) = output_file("selection-route-change-shift-tab");
    let (tx, rx) = mpsc::channel();
    let old_route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "old selection".to_string(),
            suggestion_id: "old-1".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "old selection".to_string(),
        route: old_route,
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::PromptGhost {
        text: "new selection".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![
                PromptGhostCandidate {
                    text: "new selection".to_string(),
                    suggestion_id: "new-1".to_string(),
                },
                PromptGhostCandidate {
                    text: "another selection".to_string(),
                    suggestion_id: "new-2".to_string(),
                },
            ],
            active: 0,
        },
    };

    relay_input_bytes(
        b"[Z",
        received_at + Duration::from_millis(1),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("handle shift-tab after route change");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1b[Z");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. })));
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn selection_expired_escape_dismisses_instead_of_rebuffering_for_a_new_route() {
    let (path, mut master) = output_file("selection-expired-route-change");
    let (tx, rx) = mpsc::channel();
    let old_route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "old selection".to_string(),
            suggestion_id: "old-1".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "old selection".to_string(),
        route: old_route,
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now()
        .checked_sub(Duration::from_millis(100))
        .expect("recent timestamp");
    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::PromptGhost {
        text: "new selection".to_string(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![PromptGhostCandidate {
                text: "new selection".to_string(),
                suggestion_id: "new-1".to_string(),
            }],
            active: 0,
        },
    };
    relay_input_bytes(
        b"",
        received_at + Duration::from_millis(51),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("flush expired escape");

    assert_eq!(fs::read(&path).expect("read test output"), b"\x1b");
    assert!(rx
        .try_iter()
        .any(|event| event == RawInputEvent::PromptGhostDismissed));
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::Passthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn selection_timeout_and_follow_up_byte_do_not_duplicate_or_reorder_input() {
    let (path, mut master) = output_file("selection-timeout-follow-up");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now()
        .checked_sub(Duration::from_millis(100))
        .expect("recent timestamp");

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    relay_input_bytes(
        b"x",
        received_at + Duration::from_millis(51),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("flush escape and relay follow-up");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bx");
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == RawInputEvent::PromptGhostDismissed)
            .count(),
        1
    );
    fs::remove_file(path).ok();
}

#[test]
fn selection_pending_escape_is_forwarded_when_the_input_mode_changes() {
    let (path, mut master) = output_file("selection-mode-change");
    let (tx, rx) = mpsc::channel();
    let input_mode = selection_input_mode();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    relay_input_bytes(
        b"\x1b",
        received_at,
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("buffer escape");
    *input_mode.lock().expect("input mode") = RawInputMode::RawPassthrough;
    relay_input_bytes(
        b"x",
        received_at + Duration::from_millis(1),
        &mut master,
        &tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("relay pending escape after mode change");

    let events = rx.try_iter().collect::<Vec<_>>();
    let mode = input_mode.lock().expect("input mode").clone();
    assert_eq!(fs::read(&path).expect("read test output"), b"\x1bx");
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostCycle { .. })));
    assert!(matches!(mode, RawInputMode::RawPassthrough));
    fs::remove_file(path).ok();
}

#[test]
fn shell_rewrite_tab_writes_to_native_line_editor_without_agent_intercept() {
    let (path, mut master) = output_file("native");
    let (tx, rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "grep file".to_string(),
        route: PromptGhostRoute::NativeShell,
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    assert!(relay_prompt_ghost_input(
        b"\t",
        "grep file",
        &PromptGhostRoute::NativeShell,
        &mut relay,
    )
    .expect("accept native ghost"));
    relay_passthrough_input(b"\t\x15", &mut relay)
        .expect("native completion and line clearing remain available");
    master.sync_all().expect("sync test output");

    assert_eq!(
        fs::read(&path).expect("read test output"),
        b"grep file\t\x15"
    );
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        vec![
            RawInputEvent::PromptGhostClear,
            RawInputEvent::ShellInputActivity { empty: true }
        ]
    );
    assert!(!line_buffer.force_agent_intercept);
    assert!(matches!(
        *input_mode.lock().expect("input mode"),
        RawInputMode::RawPassthrough
    ));
    fs::remove_file(path).ok();
}

#[test]
fn native_shell_input_reports_editing_then_empty_without_content() {
    let (path, mut master) = output_file("input-state");
    let (tx, rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::RawPassthrough));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_passthrough_input(b"partial", &mut relay).expect("type partial line");
    relay_passthrough_input(&[super::super::CTRL_U], &mut relay).expect("clear line");

    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        vec![
            RawInputEvent::ShellInputActivity { empty: false },
            RawInputEvent::ShellInputActivity { empty: true },
        ]
    );
    fs::remove_file(path).ok();
}

#[test]
fn agent_prompt_tab_stays_local_until_enter_and_keeps_suggestion_id() {
    let (path, mut master) = output_file("agent");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    let accepted = rx.try_iter().collect::<Vec<_>>();
    assert!(accepted.contains(&RawInputEvent::PromptGhostAccepted {
        suggestion_id: Some("suggestion-1".to_string()),
    }));
    assert!(accepted
        .iter()
        .all(|event| !matches!(event, RawInputEvent::PromptGhostIntercept { .. })));

    relay_passthrough_input(b" safely\n", &mut relay).expect("submit edited agent prompt");
    assert!(rx.try_iter().any(|event| matches!(
        event,
        RawInputEvent::PromptGhostIntercept { input, suggestion_id }
            if input == "analyze failure safely"
                && suggestion_id.as_deref() == Some("suggestion-1")
    )));
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    fs::remove_file(path).ok();
}

#[test]
fn selection_shift_tab_cycles_and_tab_inserts_the_active_prompt() {
    let relay = SelectionRelay::start("selection-cycle-tab");
    relay.send(b"\x1b[Z");
    relay.send(b"\t");

    let (events, output, mode) = relay.finish();
    assert!(events.contains(&RawInputEvent::PromptGhostCycle {
        text: "continue deployment".to_string(),
    }));
    assert!(events.contains(&RawInputEvent::PromptGhostAccepted {
        suggestion_id: Some("personal-1".to_string()),
    }));
    assert_eq!(output, b"exit\n");
    assert!(matches!(mode, RawInputMode::Passthrough));
}

#[test]
fn selection_enter_submits_the_active_prompt_without_shell_execution() {
    let (path, mut master) = output_file("selection-enter");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentSelection {
        candidates: vec![PromptGhostCandidate {
            text: "inspect disk pressure".to_string(),
            suggestion_id: "health-disk".to_string(),
        }],
        active: 0,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "inspect disk pressure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\r", "inspect disk pressure", &route, &mut relay)
        .expect("submit active selection");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        RawInputEvent::PromptGhostIntercept { input, suggestion_id }
            if input == "inspect disk pressure"
                && suggestion_id.as_deref() == Some("health-disk")
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostAccepted { .. })));
    assert_eq!(fs::read(&path).unwrap(), b"");
    assert!(matches!(*input_mode.lock().unwrap(), RawInputMode::Delay));
    fs::remove_file(path).ok();
}

#[test]
fn clearing_accepted_agent_prompt_emits_binding_dismissal() {
    let (path, mut master) = output_file("clear-agent");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    relay_passthrough_input(&[0x15], &mut relay).expect("clear accepted prompt");

    assert!(rx
        .try_iter()
        .any(|event| event == RawInputEvent::PromptGhostDismissed));
    assert!(!line_buffer.is_active());
    fs::remove_file(path).ok();
}

#[test]
fn unsupported_arrow_after_agent_prompt_tab_cancels_without_writing_to_shell() {
    let (path, mut master) = output_file("agent-arrow-cancel");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    relay_passthrough_input(b"\x1b[D", &mut relay).expect("cancel unsupported edit");
    master.sync_all().expect("sync test output");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(fs::read(&path).expect("read test output"), b"");
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
    assert!(!line_buffer.is_active());
    assert!(line_buffer.forced_agent_suggestion_id.is_none());
    fs::remove_file(path).ok();
}

#[test]
fn split_cursor_sequences_after_agent_prompt_tab_never_reach_shell() {
    for (name, sequence) in [
        ("left", b"\x1b[D".as_slice()),
        ("right", b"\x1b[C".as_slice()),
        ("home", b"\x1b[H".as_slice()),
        ("end", b"\x1b[F".as_slice()),
    ] {
        let (path, mut master) = output_file(&format!("agent-split-{name}"));
        let (tx, rx) = mpsc::channel();
        let route = PromptGhostRoute::AgentIntercept {
            suggestion_id: Some("suggestion-1".to_string()),
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
            text: "analyze failure".to_string(),
            route: route.clone(),
        }));
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let classifier = InputClassifier::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &tx,
            input_mode: &input_mode,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
        };

        relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
            .expect("accept agent ghost");
        for byte in sequence {
            relay_passthrough_input(&[*byte], &mut relay).expect("relay split sequence");
        }
        master.sync_all().expect("sync test output");

        let events = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(fs::read(&path).expect("read test output"), b"");
        assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
        assert!(!events
            .iter()
            .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
        assert!(!line_buffer.is_active());
        fs::remove_file(path).ok();
    }
}

#[test]
fn clearing_and_submitting_in_one_buffer_dismisses_binding() {
    let (path, mut master) = output_file("clear-submit-agent");
    let (tx, rx) = mpsc::channel();
    let route = PromptGhostRoute::AgentIntercept {
        suggestion_id: Some("suggestion-1".to_string()),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: "analyze failure".to_string(),
        route: route.clone(),
    }));
    let mut line_buffer = CandidateLineBuffer::default();
    let mut native_line_state = NativeLineState::default();
    let mut exit_tracker = ExplicitExitTracker::default();
    let classifier = InputClassifier::default();
    let mut relay = InputRelayContext {
        master: &mut master,
        input_classifier: &classifier,
        input_events: &tx,
        input_mode: &input_mode,
        line_buffer: &mut line_buffer,
        native_line_state: &mut native_line_state,
        exit_tracker: &mut exit_tracker,
    };

    relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
        .expect("accept agent ghost");
    relay_passthrough_input(b"\x15\n", &mut relay).expect("clear and submit");

    let events = rx.try_iter().collect::<Vec<_>>();
    assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
    assert!(line_buffer.forced_agent_suggestion_id.is_none());
    assert_eq!(fs::read(&path).expect("read test output"), b"\n");
    fs::remove_file(path).ok();
}
