use std::fs::{self, OpenOptions};

use super::super::{PromptGhostCandidate, RawInputCapture};
use super::*;

#[test]
fn delayed_ghost_suffix_keeps_capture_generation_across_replacement() {
    let path = std::env::temp_dir().join(format!(
        "cosh-shell-delayed-ghost-suffix-{}",
        std::process::id()
    ));
    let mut master = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .expect("test output file");
    let (input_tx, input_rx) = mpsc::channel();
    let previous = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let next = RawInputCapture::Question {
        id: "q-2".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let candidate = PromptGhostCandidate {
        text: "inspect memory".to_string(),
        suggestion_id: "health-1".to_string(),
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
        text: candidate.text.clone(),
        route: PromptGhostRoute::AgentSelection {
            candidates: vec![candidate],
            active: 0,
        },
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState::default();
    let received_at = Instant::now();

    relay_input_bytes_with_read_ahead(
        b"\x1b",
        received_at,
        &mut master,
        &input_tx,
        &classifier,
        &input_mode,
        &mut state,
        RelayReadContext::default(),
    )
    .expect("buffer ghost escape");
    *input_mode.lock().expect("input mode") = RawInputMode::Capture {
        capture: previous.clone(),
        generation: 7,
        installed_at: Instant::now(),
    };
    relay_input_bytes_with_read_ahead(
        b"[",
        received_at + Duration::from_millis(1),
        &mut master,
        &input_tx,
        &classifier,
        &input_mode,
        &mut state,
        RelayReadContext {
            read_ahead: None,
            expected_capture_generation: Some(7),
        },
    )
    .expect("buffer partial replaced ghost suffix");
    *input_mode.lock().expect("input mode") = RawInputMode::Draining {
        previous_capture: previous,
        generation: 7,
        next_capture: Some(next),
        invalidated: false,
    };
    let mode = current_raw_input_mode(&input_mode);
    flush_pending_replaced_prompt_ghost_suffix(
        true,
        received_at + Duration::from_millis(60),
        &mode,
        &mut master,
        &input_tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("flush partial suffix");
    finish_input_relay(&mut master, &input_tx, &classifier, &input_mode, &mut state)
        .expect("finish relay");

    let events = input_rx.try_iter().collect::<Vec<_>>();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RawInputEvent::CardInput(target, _) if target == "q-2"
        )),
        "{events:?}"
    );
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn ghost_suffix_does_not_consume_input_from_a_new_capture_generation() {
    let path = std::env::temp_dir().join(format!(
        "cosh-shell-ghost-suffix-generation-boundary-{}",
        std::process::id()
    ));
    let mut master = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .expect("test output file");
    let (input_tx, input_rx) = mpsc::channel();
    let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
        capture: RawInputCapture::Question {
            id: "q-2".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        },
        generation: 8,
        installed_at: Instant::now(),
    }));
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState {
        pending_replaced_prompt_ghost_suffix: Some(PendingReplacedPromptGhostSuffix {
            bytes: b"[".to_vec(),
            deadline: Instant::now() + Duration::from_millis(50),
            expected_capture_generation: Some(7),
        }),
        ..RawInputRelayState::default()
    };

    relay_input_bytes_with_read_ahead(
        b"Z",
        Instant::now(),
        &mut master,
        &input_tx,
        &classifier,
        &input_mode,
        &mut state,
        RelayReadContext {
            read_ahead: None,
            expected_capture_generation: Some(8),
        },
    )
    .expect("relay new generation input");

    let events = input_rx.try_iter().collect::<Vec<_>>();
    assert!(
        events.iter().any(
            |event| matches!(event, RawInputEvent::CardInput(target, input) if target == "q-2" && input == "Z")
        ),
        "{events:?}"
    );
    finish_input_relay(&mut master, &input_tx, &classifier, &input_mode, &mut state)
        .expect("finish relay");
    master.sync_all().expect("sync test output");
    assert_eq!(fs::read(&path).expect("read test output"), b"exit\n");
    fs::remove_file(path).ok();
}

#[test]
fn delay_escape_does_not_cancel_a_later_capture() {
    let path = std::env::temp_dir().join(format!(
        "cosh-shell-delay-escape-capture-boundary-{}",
        std::process::id()
    ));
    let mut master = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .expect("test output file");
    let capture = RawInputCapture::Question {
        id: "new-question".to_string(),
        option_count: 0,
        allow_free_text: true,
        multiple: false,
        secret: false,
    };
    let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
        capture: capture.clone(),
        generation: 42,
        installed_at: Instant::now(),
    }));
    let (input_tx, input_rx) = mpsc::channel();
    let classifier = InputClassifier::default();
    let mut state = RawInputRelayState {
        pending_delay_escape: Some(PendingDelayEscape {
            bytes: vec![ESC],
            deadline: Instant::now(),
            generation: 7,
        }),
        ..RawInputRelayState::default()
    };

    flush_pending_delay_escape(
        true,
        Instant::now(),
        &mut master,
        &input_tx,
        &classifier,
        &input_mode,
        &mut state,
    )
    .expect("flush stale delay escape");

    assert!(input_rx.try_iter().next().is_none());
    assert!(matches!(
        current_raw_input_mode(&input_mode),
        RawInputMode::Capture {
            capture: active,
            generation: 42,
            ..
        } if active == capture
    ));
    master.sync_all().expect("sync test output");
    assert!(fs::read(&path).expect("read test output").is_empty());
    fs::remove_file(path).ok();
}
