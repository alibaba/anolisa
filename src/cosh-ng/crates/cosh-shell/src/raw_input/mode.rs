use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::types::ShellHandoffRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawObserverAction {
    Continue,
    RawPassthrough,
    HoldShellOutput,
    DelayShellOutput,
    CaptureInput(RawInputCapture),
    EmitToPty(ShellHandoffRequest),
    EmitToPtyWithPromptRestore(ShellHandoffRequest),
    InterruptForeground,
    RestorePrompt {
        ghost_text: Option<String>,
        ghost_route: PromptGhostRoute,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptGhostRoute {
    NativeShell,
    AgentIntercept {
        suggestion_id: Option<String>,
    },
    AgentSelection {
        candidates: Vec<PromptGhostCandidate>,
        active: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptGhostCandidate {
    pub text: String,
    pub suggestion_id: String,
}

impl Default for PromptGhostRoute {
    fn default() -> Self {
        Self::AgentIntercept {
            suggestion_id: None,
        }
    }
}

impl RawObserverAction {
    pub(crate) fn hold_shell_output(self) -> bool {
        matches!(
            self,
            Self::HoldShellOutput | Self::DelayShellOutput | Self::CaptureInput(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawInputMode {
    Passthrough,
    RawPassthrough,
    Hold,
    Delay {
        generation: u64,
    },
    PromptGhost {
        text: String,
        route: PromptGhostRoute,
    },
    Capture {
        capture: RawInputCapture,
        generation: u64,
        installed_at: std::time::Instant,
    },
    Submitted {
        capture: RawInputCapture,
        generation: u64,
    },
    Draining {
        previous_capture: RawInputCapture,
        generation: u64,
        next_capture: Option<RawInputCapture>,
        invalidated: bool,
    },
    Terminal {
        previous_capture: RawInputCapture,
        generation: u64,
    },
}

static CAPTURE_GENERATION: AtomicU64 = AtomicU64::new(1);
static DELAY_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(super) fn new_delay_input_mode() -> RawInputMode {
    RawInputMode::Delay {
        generation: DELAY_GENERATION.fetch_add(1, Ordering::Relaxed),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawInputCapture {
    Question {
        id: String,
        option_count: usize,
        allow_free_text: bool,
        multiple: bool,
        secret: bool,
    },
    Approval {
        id: String,
        is_hook: bool,
    },
    Mode {
        id: String,
        option_count: usize,
        selected: usize,
    },
    Config {
        id: String,
        option_count: usize,
        selected: usize,
    },
    ConfigLanguage {
        id: String,
        option_count: usize,
        selected: usize,
    },
    Session {
        id: String,
        option_count: usize,
        selected: usize,
        confirming_clear: bool,
    },
    Consultation {
        id: String,
    },
    Evidence {
        id: String,
    },
}

pub(crate) fn update_input_mode(
    input_mode: &Arc<Mutex<RawInputMode>>,
    action: &RawObserverAction,
    acknowledged_generation: Option<u64>,
) {
    let Ok(mut mode) = input_mode.lock() else {
        return;
    };
    if matches!(
        action,
        RawObserverAction::Continue | RawObserverAction::RawPassthrough
    ) && matches!(&*mode, RawInputMode::PromptGhost { .. })
    {
        return;
    }
    if let RawInputMode::Submitted {
        capture,
        generation,
    } = &*mode
    {
        if acknowledged_generation != Some(*generation) {
            return;
        }
        if matches!(action, RawObserverAction::CaptureInput(active) if active == capture) {
            return;
        }
        let next_capture = match action {
            RawObserverAction::CaptureInput(next) => Some(next.clone()),
            _ => None,
        };
        *mode = RawInputMode::Draining {
            previous_capture: capture.clone(),
            generation: *generation,
            next_capture,
            invalidated: false,
        };
        return;
    }
    if let RawInputMode::Draining {
        previous_capture,
        next_capture,
        invalidated,
        generation,
        ..
    } = &mut *mode
    {
        match action {
            RawObserverAction::CaptureInput(next)
                if next != previous_capture
                    && !(*invalidated && acknowledged_generation == Some(*generation)) =>
            {
                *next_capture = Some(next.clone());
            }
            RawObserverAction::CaptureInput(_) => {}
            _ => *next_capture = None,
        }
        return;
    }
    if let RawInputMode::Terminal {
        previous_capture,
        generation,
    } = &*mode
    {
        if matches!(action, RawObserverAction::CaptureInput(next) if next == previous_capture)
            || (matches!(action, RawObserverAction::CaptureInput(_))
                && acknowledged_generation == Some(*generation))
        {
            return;
        }
    }
    if let RawInputMode::Capture {
        capture,
        generation,
        ..
    } = &*mode
    {
        if matches!(action, RawObserverAction::CaptureInput(active) if active == capture) {
            return;
        }
        if let RawObserverAction::CaptureInput(next_capture) = action {
            *mode = RawInputMode::Capture {
                capture: next_capture.clone(),
                generation: CAPTURE_GENERATION.fetch_add(1, Ordering::Relaxed),
                installed_at: std::time::Instant::now(),
            };
            return;
        }
        *mode = RawInputMode::Draining {
            previous_capture: capture.clone(),
            generation: *generation,
            next_capture: None,
            invalidated: false,
        };
        return;
    }
    *mode = match action {
        RawObserverAction::CaptureInput(capture) => RawInputMode::Capture {
            capture: capture.clone(),
            generation: CAPTURE_GENERATION.fetch_add(1, Ordering::Relaxed),
            installed_at: std::time::Instant::now(),
        },
        RawObserverAction::HoldShellOutput => RawInputMode::Hold,
        RawObserverAction::DelayShellOutput => new_delay_input_mode(),
        RawObserverAction::RawPassthrough => RawInputMode::RawPassthrough,
        RawObserverAction::RestorePrompt {
            ghost_text: Some(text),
            ghost_route,
        } => RawInputMode::PromptGhost {
            text: text.clone(),
            route: ghost_route.clone(),
        },
        RawObserverAction::Continue
        | RawObserverAction::EmitToPty(_)
        | RawObserverAction::EmitToPtyWithPromptRestore(_)
        | RawObserverAction::InterruptForeground
        | RawObserverAction::RestorePrompt {
            ghost_text: None, ..
        } => RawInputMode::Passthrough,
    };
}

pub(crate) fn current_raw_input_mode(input_mode: &Arc<Mutex<RawInputMode>>) -> RawInputMode {
    input_mode
        .lock()
        .map(|mode| mode.clone())
        .unwrap_or(RawInputMode::Passthrough)
}

pub(crate) fn submit_capture(
    input_mode: &Arc<Mutex<RawInputMode>>,
    expected_capture: &RawInputCapture,
) -> Option<u64> {
    let mut mode = input_mode.lock().ok()?;
    let RawInputMode::Capture {
        capture,
        generation,
        ..
    } = &*mode
    else {
        return None;
    };
    if capture != expected_capture {
        return None;
    }
    let generation = *generation;
    *mode = RawInputMode::Submitted {
        capture: capture.clone(),
        generation,
    };
    Some(generation)
}

pub(crate) fn complete_capture_chain_if_pending(
    input_mode: &Arc<Mutex<RawInputMode>>,
    generation: u64,
) -> bool {
    let Ok(mut mode) = input_mode.lock() else {
        return false;
    };
    let RawInputMode::Draining {
        generation: active,
        next_capture,
        invalidated,
        ..
    } = &*mode
    else {
        return false;
    };
    if *active != generation || *invalidated {
        return false;
    }
    let Some(next_capture) = next_capture.clone() else {
        return false;
    };
    *mode = RawInputMode::Capture {
        capture: next_capture,
        generation: CAPTURE_GENERATION.fetch_add(1, Ordering::Relaxed),
        installed_at: std::time::Instant::now(),
    };
    true
}

pub(crate) fn complete_capture_replay(input_mode: &Arc<Mutex<RawInputMode>>, generation: u64) {
    let Ok(mut mode) = input_mode.lock() else {
        return;
    };
    let RawInputMode::Draining {
        previous_capture,
        generation: active,
        next_capture,
        ..
    } = &*mode
    else {
        return;
    };
    if *active != generation {
        return;
    }
    let previous_capture = previous_capture.clone();
    *mode = if let Some(capture) = next_capture.clone() {
        RawInputMode::Capture {
            capture,
            generation: CAPTURE_GENERATION.fetch_add(1, Ordering::Relaxed),
            installed_at: std::time::Instant::now(),
        }
    } else {
        RawInputMode::Terminal {
            previous_capture,
            generation,
        }
    };
}

pub(crate) fn expire_capture_submission(input_mode: &Arc<Mutex<RawInputMode>>, generation: u64) {
    if let Ok(mut mode) = input_mode.lock() {
        if matches!(&*mode, RawInputMode::Submitted { generation: active, .. } if *active == generation)
        {
            let RawInputMode::Submitted { capture, .. } = &*mode else {
                return;
            };
            *mode = RawInputMode::Draining {
                previous_capture: capture.clone(),
                generation,
                next_capture: None,
                invalidated: true,
            };
        } else if let RawInputMode::Draining {
            generation: active,
            next_capture,
            invalidated,
            ..
        } = &mut *mode
        {
            if *active == generation {
                *next_capture = None;
                *invalidated = true;
            }
        }
    }
}

pub(crate) fn abandon_active_capture(input_mode: &Arc<Mutex<RawInputMode>>) {
    if let Ok(mut mode) = input_mode.lock() {
        if let RawInputMode::Capture {
            capture,
            generation,
            ..
        } = &*mode
        {
            *mode = RawInputMode::Draining {
                previous_capture: capture.clone(),
                generation: *generation,
                next_capture: None,
                invalidated: true,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_lease_waits_for_ack_and_drains_before_replacement() {
        let state = Arc::new(Mutex::new(RawInputMode::Passthrough));
        let capture = RawInputCapture::Consultation {
            id: "consult-1".to_string(),
        };
        let action = RawObserverAction::CaptureInput(capture.clone());

        update_input_mode(&state, &action, None);
        let RawInputMode::Capture { generation, .. } = current_raw_input_mode(&state) else {
            panic!("capture lease");
        };
        update_input_mode(&state, &action, None);
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Capture {
                generation: active,
                ..
            } if active == generation
        ));

        assert_eq!(submit_capture(&state, &capture), Some(generation));
        update_input_mode(&state, &action, Some(generation.saturating_add(1)));
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Submitted {
                generation: active,
                ..
            } if active == generation
        ));

        let next_capture = RawInputCapture::Consultation {
            id: "consult-2".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(next_capture.clone()),
            Some(generation),
        );
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Draining {
                previous_capture: ref previous,
                generation: active,
                next_capture: Some(ref next),
                ..
            } if previous == &capture && active == generation && next == &next_capture
        ));
        let replacement = RawObserverAction::CaptureInput(next_capture.clone());
        update_input_mode(&state, &replacement, Some(generation));
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Draining {
                generation: active,
                next_capture: Some(ref active_capture),
                ..
            } if active == generation && active_capture == &next_capture
        ));

        assert!(complete_capture_chain_if_pending(&state, generation));
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Capture {
                capture: active,
                generation: active_generation,
                ..
            } if active == next_capture && active_generation != generation
        ));
    }

    #[test]
    fn active_capture_replacement_is_ready_without_an_extra_input() {
        let state = Arc::new(Mutex::new(RawInputMode::Passthrough));
        let picker = RawInputCapture::Session {
            id: "session-panel".to_string(),
            option_count: 2,
            selected: 0,
            confirming_clear: false,
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(picker.clone()),
            None,
        );
        let RawInputMode::Capture {
            generation: picker_generation,
            ..
        } = current_raw_input_mode(&state)
        else {
            panic!("session picker capture");
        };
        let confirmation = RawInputCapture::Session {
            id: "session-panel".to_string(),
            option_count: 2,
            selected: 0,
            confirming_clear: true,
        };

        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(confirmation.clone()),
            None,
        );

        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Capture {
                capture,
                generation,
                ..
            } if capture == confirmation && generation != picker_generation
        ));
    }

    #[test]
    fn expired_capture_rejects_old_and_accepts_new_target_after_drain() {
        let state = Arc::new(Mutex::new(RawInputMode::Passthrough));
        let capture = RawInputCapture::Consultation {
            id: "consult-1".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(capture.clone()),
            None,
        );
        let generation = submit_capture(&state, &capture).expect("submitted capture");
        expire_capture_submission(&state, generation);
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(capture.clone()),
            Some(generation),
        );
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Draining {
                generation: active,
                next_capture: None,
                ..
            } if active == generation
        ));
        assert!(!complete_capture_chain_if_pending(&state, generation));
        let next_capture = RawInputCapture::Consultation {
            id: "consult-2".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(next_capture.clone()),
            Some(generation),
        );
        assert!(!complete_capture_chain_if_pending(&state, generation));
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(next_capture.clone()),
            None,
        );
        assert!(!complete_capture_chain_if_pending(&state, generation));
        complete_capture_replay(&state, generation);
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Capture { capture: active, .. } if active == next_capture
        ));
    }

    #[test]
    fn terminal_rejects_old_target_and_allows_new_target() {
        let state = Arc::new(Mutex::new(RawInputMode::Passthrough));
        let capture = RawInputCapture::Consultation {
            id: "consult-1".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(capture.clone()),
            None,
        );
        let generation = submit_capture(&state, &capture).expect("submitted capture");
        update_input_mode(&state, &RawObserverAction::Continue, Some(generation));
        assert!(!complete_capture_chain_if_pending(&state, generation));
        complete_capture_replay(&state, generation);

        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(capture.clone()),
            Some(generation),
        );
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Terminal {
                generation: active,
                ..
            } if active == generation
        ));

        let next_capture = RawInputCapture::Consultation {
            id: "consult-2".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(next_capture.clone()),
            Some(generation),
        );
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Terminal { .. }
        ));
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(next_capture.clone()),
            None,
        );
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Capture { capture: active, .. } if active == next_capture
        ));
    }

    #[test]
    fn drain_completion_uses_live_pending_target() {
        let state = Arc::new(Mutex::new(RawInputMode::Passthrough));
        let capture = RawInputCapture::Consultation {
            id: "consult-a".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(capture.clone()),
            None,
        );
        let generation = submit_capture(&state, &capture).expect("submitted capture");
        let target_b = RawInputCapture::Consultation {
            id: "consult-b".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(target_b),
            Some(generation),
        );
        update_input_mode(&state, &RawObserverAction::Continue, Some(generation));

        assert!(!complete_capture_chain_if_pending(&state, generation));
        complete_capture_replay(&state, generation);
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Terminal { .. }
        ));

        update_input_mode(&state, &RawObserverAction::Continue, Some(generation));
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(capture.clone()),
            None,
        );
        let generation = submit_capture(&state, &capture).expect("submitted capture");
        let target_b = RawInputCapture::Consultation {
            id: "consult-b".to_string(),
        };
        let target_c = RawInputCapture::Consultation {
            id: "consult-c".to_string(),
        };
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(target_b),
            Some(generation),
        );
        update_input_mode(
            &state,
            &RawObserverAction::CaptureInput(target_c.clone()),
            Some(generation),
        );

        assert!(complete_capture_chain_if_pending(&state, generation));
        assert!(matches!(
            current_raw_input_mode(&state),
            RawInputMode::Capture { capture: active, .. } if active == target_c
        ));
    }
}
