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
    InterruptForeground,
    RestorePrompt { ghost_text: Option<String> },
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
    Delay,
    PromptGhost(String),
    Capture(RawInputCapture),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawInputCapture {
    Question {
        id: String,
        option_count: usize,
        allow_free_text: bool,
        multiple: bool,
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
    Consultation {
        id: String,
    },
    Evidence {
        id: String,
    },
}

pub(crate) fn update_input_mode(input_mode: &Arc<Mutex<RawInputMode>>, action: &RawObserverAction) {
    let Ok(mut mode) = input_mode.lock() else {
        return;
    };
    if matches!(
        action,
        RawObserverAction::Continue | RawObserverAction::RawPassthrough
    ) && matches!(&*mode, RawInputMode::PromptGhost(_))
    {
        return;
    }
    *mode = match action {
        RawObserverAction::CaptureInput(capture) => RawInputMode::Capture(capture.clone()),
        RawObserverAction::HoldShellOutput => RawInputMode::Hold,
        RawObserverAction::DelayShellOutput => RawInputMode::Delay,
        RawObserverAction::RawPassthrough => RawInputMode::RawPassthrough,
        RawObserverAction::RestorePrompt {
            ghost_text: Some(text),
        } => RawInputMode::PromptGhost(text.clone()),
        RawObserverAction::Continue
        | RawObserverAction::EmitToPty(_)
        | RawObserverAction::InterruptForeground
        | RawObserverAction::RestorePrompt { ghost_text: None } => RawInputMode::Passthrough,
    };
}

pub(crate) fn current_raw_input_mode(input_mode: &Arc<Mutex<RawInputMode>>) -> RawInputMode {
    input_mode
        .lock()
        .map(|mode| mode.clone())
        .unwrap_or(RawInputMode::Passthrough)
}
