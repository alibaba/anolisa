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
        // A stale snapshot must not wipe live selection state: when the
        // live mode still captures a card (e.g. the same approval card
        // re-armed under a new generation after an action-set switch),
        // re-align the card state to the live capture so apply_capture's
        // remap keeps the highlighted action; a different card resets
        // inside apply_capture, and only a non-capture mode drops the
        // state entirely.
        if let RawInputMode::Capture {
            capture: active, ..
        } = &*mode
        {
            card_state.apply_capture(active);
        } else {
            card_state.reset();
        }
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
            | RawInputEvent::CardApproveTurn(_)
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

    /// Stale-generation 竞态回归（评审 P1 追加）：旧 generation 的 Standard
    /// snapshot 到达时，live 模式已把同一张卡切到 TurnConsent。mismatch
    /// 分支必须对齐到 live capture（重映射保留选择）而不是清空，否则重试
    /// 后回车会从 index 0 发出 CardApprove，与仍高亮 Deny 的卡面错位。
    #[test]
    fn stale_approval_snapshot_realigns_to_live_action_set() {
        let standard = RawInputCapture::Approval {
            id: "req-1".to_string(),
            action_set: crate::ui::ApprovalActionSet::Standard,
        };
        let turn = RawInputCapture::Approval {
            id: "req-1".to_string(),
            action_set: crate::ui::ApprovalActionSet::TurnConsent,
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::Capture {
            capture: turn.clone(),
            generation: 8,
            installed_at: std::time::Instant::now(),
        }));
        let (sender, receiver) = mpsc::channel();
        let mut state = CardInputState::default();
        // 在旧 generation 的 Standard 快照下选中 Deny（index 2）。
        state.apply_capture(&standard);
        state.consume(&standard, b"\x1b[C\x1b[C");

        // 旧快照到达：mismatch 分支对齐到 live TurnConsent，不清选择。
        let result = consume_captured_input(&mut state, &standard, 7, b"\n", &sender, &input_mode);
        assert!(result.retry);

        // 重试换用 live 快照：回车提交的仍是重映射后的 Deny。
        let result = consume_captured_input(&mut state, &turn, 8, b"\n", &sender, &input_mode);
        assert!(!result.retry);
        let events: Vec<_> = receiver.try_iter().collect();
        assert!(
            events.contains(&RawInputEvent::CardDeny("req-1".to_string())),
            "expected remapped Deny, got {events:?}"
        );
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
