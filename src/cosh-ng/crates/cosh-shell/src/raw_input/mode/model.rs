//! Raw-input mode state and observer action types.

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

/// Input ownership boundary for a [`RawInputMode`]. Display-only updates
/// inside the same owner (prompt ghost candidate cycling, card selection
/// redraws) keep the owner stable, so bytes obtained across such updates are
/// not treated as an ownership cutover and never get silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputOwnership {
    Passthrough,
    RawPassthrough,
    Hold,
    Delay(u64),
    PromptGhost,
    Capture(u64),
    Submitted(u64),
    Draining(u64),
    Terminal(u64),
}

impl RawInputMode {
    pub(crate) fn input_ownership(&self) -> InputOwnership {
        match self {
            Self::Passthrough => InputOwnership::Passthrough,
            Self::RawPassthrough => InputOwnership::RawPassthrough,
            Self::Hold => InputOwnership::Hold,
            Self::Delay { generation } => InputOwnership::Delay(*generation),
            Self::PromptGhost { .. } => InputOwnership::PromptGhost,
            Self::Capture { generation, .. } => InputOwnership::Capture(*generation),
            Self::Submitted { generation, .. } => InputOwnership::Submitted(*generation),
            Self::Draining { generation, .. } => InputOwnership::Draining(*generation),
            Self::Terminal { generation, .. } => InputOwnership::Terminal(*generation),
        }
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
        action_set: crate::ui::ApprovalActionSet,
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
