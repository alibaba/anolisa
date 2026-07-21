use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use super::card_capture::CardInputState;
use super::{RawInputCapture, RawInputEvent, RawInputMode};

pub(super) fn consume_captured_input(
    card_state: &mut CardInputState,
    capture: &RawInputCapture,
    bytes: &[u8],
    input_events: &Sender<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
) -> bool {
    card_state.apply_capture(capture);
    let events = card_state.consume(capture, bytes);
    let released = events.iter().any(releases_mode_capture);
    if released {
        if let Ok(mut mode) = input_mode.lock() {
            *mode = RawInputMode::Passthrough;
        }
        card_state.reset();
    }
    for event in events {
        let _ = input_events.send(event);
    }
    released
}

fn releases_mode_capture(event: &RawInputEvent) -> bool {
    matches!(
        event,
        RawInputEvent::ModeSet(_, _)
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
