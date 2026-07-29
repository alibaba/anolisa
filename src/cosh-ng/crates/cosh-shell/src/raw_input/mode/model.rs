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

/// Semantic destination of a [`PromptGhostRoute`], stripped of display-only
/// payload (ghost text, candidate list, active index). Routes with the same
/// kind interpret Tab/Enter identically, so kind changes mark an input
/// ownership cutover while same-kind refreshes do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptGhostRouteKind {
    NativeShell,
    AgentIntercept,
    AgentSelection,
}

impl PromptGhostRoute {
    /// Exhaustive on purpose (no wildcard arm): adding a `PromptGhostRoute`
    /// variant fails to compile here until the new variant receives its own
    /// ownership classification, so a future route can never be silently
    /// folded into an existing kind.
    pub(crate) fn kind(&self) -> PromptGhostRouteKind {
        match self {
            Self::NativeShell => PromptGhostRouteKind::NativeShell,
            Self::AgentIntercept { .. } => PromptGhostRouteKind::AgentIntercept,
            Self::AgentSelection { .. } => PromptGhostRouteKind::AgentSelection,
        }
    }
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
/// A prompt ghost route change (e.g. `AgentSelection` -> `NativeShell`) is
/// not display-only: Tab/Enter have different destinations per route kind,
/// so crossing kinds is an ownership cutover and in-flight bytes are
/// discarded instead of being reinterpreted under the new route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputOwnership {
    Passthrough,
    RawPassthrough,
    Hold,
    Delay(u64),
    PromptGhost(PromptGhostRouteKind),
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
            Self::PromptGhost { route, .. } => InputOwnership::PromptGhost(route.kind()),
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
    /// Editable text-only question whose input buffer starts with `initial_text`.
    TextQuestion {
        id: String,
        initial_text: String,
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
    /// Multi-line prompt draft card (#1721 D13): opened by the first soft
    /// newline in a native/escape candidate; the capture owns every key
    /// until submit (Enter) or cancel (Esc/Ctrl+C).
    PromptDraft {
        id: String,
        initial_text: String,
    },
}
