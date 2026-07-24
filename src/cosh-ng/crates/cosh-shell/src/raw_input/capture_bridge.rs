use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use super::card_capture::CardInputState;
use super::mode::RawInputMode;
use super::{RawInputCapture, RawInputEvent};

pub(super) struct CaptureConsumeResult {
    pub(super) generation: Option<u64>,
    pub(super) remainder: Vec<u8>,
    pub(super) retry: bool,
}

pub(super) fn consume_captured_input(
    card_state: &mut CardInputState,
    capture: &RawInputCapture,
    generation: u64,
    bytes: &[u8],
    input_events: &Sender<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
) -> CaptureConsumeResult {
    let Ok(mut mode) = input_mode.lock() else {
        return CaptureConsumeResult {
            generation: None,
            remainder: bytes.to_vec(),
            retry: true,
        };
    };
    if !matches!(
        &*mode,
        RawInputMode::Capture {
            capture: active,
            generation: active_generation,
            ..
        } if active == capture && *active_generation == generation
    ) {
        card_state.reset();
        return CaptureConsumeResult {
            generation: None,
            remainder: bytes.to_vec(),
            retry: true,
        };
    }
    card_state.apply_capture(capture);
    let (events, remainder) = card_state.consume_split(capture, bytes);
    let released = events.iter().any(releases_mode_capture);
    let submitted_generation = released.then_some(generation);
    if released {
        *mode = RawInputMode::Submitted {
            capture: capture.clone(),
            generation,
        };
        card_state.reset();
    }
    drop(mode);
    if let Some(generation) = submitted_generation {
        let (kind, target_id) = capture_target(capture);
        let _ = input_events.send(RawInputEvent::CaptureSubmitted {
            kind,
            target_id: target_id.to_string(),
            generation,
        });
    }
    for event in events {
        let _ = input_events.send(event);
    }
    CaptureConsumeResult {
        generation: submitted_generation,
        remainder,
        retry: false,
    }
}

fn capture_target(capture: &RawInputCapture) -> (&'static str, &str) {
    match capture {
        RawInputCapture::Question { id, .. } => ("question", id),
        RawInputCapture::Approval { id, .. } => ("approval", id),
        RawInputCapture::Mode { id, .. } => ("mode", id),
        RawInputCapture::Config { id, .. } => ("config", id),
        RawInputCapture::ConfigLanguage { id, .. } => ("config_language", id),
        RawInputCapture::Session { id, .. } => ("session", id),
        RawInputCapture::Consultation { id } => ("consultation", id),
        RawInputCapture::Evidence { id } => ("evidence", id),
    }
}

fn releases_mode_capture(event: &RawInputEvent) -> bool {
    matches!(
        event,
        RawInputEvent::CardApprove(_)
            | RawInputEvent::CardAlwaysTrust(_)
            | RawInputEvent::CardDeny(_)
            | RawInputEvent::CardCancel(_)
            | RawInputEvent::CardAnswer(_)
            | RawInputEvent::CardSecretAnswer(_)
            | RawInputEvent::ModeSet(_, _)
            | RawInputEvent::ModeCancel(_)
            | RawInputEvent::ConfigSave(_)
            | RawInputEvent::ConfigCancel(_)
            | RawInputEvent::ConfigLanguageSet(_, _)
            | RawInputEvent::ConfigLanguageCancel(_)
            | RawInputEvent::SessionResume(_, _)
            | RawInputEvent::SessionClearConfirm(_)
            | RawInputEvent::SessionCancel(_)
            | RawInputEvent::QuestionCancel(_)
            | RawInputEvent::EvidenceSend(_)
            | RawInputEvent::EvidenceIgnore(_)
            | RawInputEvent::EvidenceCancel(_)
    )
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn question() -> RawInputCapture {
        RawInputCapture::Question {
            id: "question-1".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        }
    }

    #[test]
    fn stale_capture_snapshot_retries_the_complete_chunk() {
        let capture = question();
        let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
            capture: capture.clone(),
            generation: 7,
            installed_at: std::time::Instant::now(),
        }));
        let (sender, receiver) = mpsc::channel();
        let mut state = CardInputState::default();

        let result =
            consume_captured_input(&mut state, &capture, 6, b"answer\n", &sender, &input_mode);

        assert!(result.retry);
        assert_eq!(result.remainder, b"answer\n");
        assert!(receiver.try_recv().is_err());
        assert!(matches!(
            &*input_mode.lock().expect("input mode"),
            RawInputMode::Capture { generation: 7, .. }
        ));
    }

    #[test]
    fn matching_capture_submits_under_the_same_lock() {
        let capture = question();
        let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
            capture: capture.clone(),
            generation: 7,
            installed_at: std::time::Instant::now(),
        }));
        let (sender, _receiver) = mpsc::channel();
        let mut state = CardInputState::default();

        let result =
            consume_captured_input(&mut state, &capture, 7, b"answer\n", &sender, &input_mode);

        assert!(!result.retry);
        assert_eq!(result.generation, Some(7));
        assert!(matches!(
            &*input_mode.lock().expect("input mode"),
            RawInputMode::Submitted { generation: 7, .. }
        ));
    }
}
