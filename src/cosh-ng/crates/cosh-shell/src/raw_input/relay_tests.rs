use std::fs::{self, OpenOptions};
use std::sync::{mpsc, Arc, Mutex};

use super::super::PromptGhostCandidate;
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
    let (path, mut master) = output_file("selection-cycle-tab");
    let (tx, rx) = mpsc::channel();
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
    let route = PromptGhostRoute::AgentSelection {
        candidates: candidates.clone(),
        active: 0,
        pending_escape: Vec::new(),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: candidates[0].text.clone(),
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

    relay_prompt_ghost_input(b"\x1b", &candidates[0].text, &route, &mut relay)
        .expect("buffer shift-tab escape");
    let paused = match input_mode.lock().unwrap().clone() {
        RawInputMode::PromptGhost { route, .. } => route,
        mode => panic!("expected prompt ghost, got {mode:?}"),
    };
    relay_prompt_ghost_input(b"[", &candidates[0].text, &paused, &mut relay)
        .expect("buffer shift-tab bracket");
    let paused = match input_mode.lock().unwrap().clone() {
        RawInputMode::PromptGhost { route, .. } => route,
        mode => panic!("expected prompt ghost, got {mode:?}"),
    };
    relay_prompt_ghost_input(b"Z", &candidates[0].text, &paused, &mut relay)
        .expect("cycle split selection");
    let cycled_route = PromptGhostRoute::AgentSelection {
        candidates,
        active: 1,
        pending_escape: Vec::new(),
    };
    relay_prompt_ghost_input(b"\t", "continue deployment", &cycled_route, &mut relay)
        .expect("insert active selection");

    assert_eq!(line_buffer.visible_line_bytes(), b"continue deployment");
    assert_eq!(
        line_buffer.forced_agent_suggestion_id.as_deref(),
        Some("personal-1")
    );
    let events = rx.try_iter().collect::<Vec<_>>();
    assert!(events.contains(&RawInputEvent::PromptGhostCycle {
        text: "continue deployment".to_string(),
    }));
    assert!(events.contains(&RawInputEvent::PromptGhostAccepted {
        suggestion_id: Some("personal-1".to_string()),
    }));
    assert_eq!(fs::read(&path).unwrap(), b"");
    fs::remove_file(path).ok();
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
        pending_escape: Vec::new(),
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
