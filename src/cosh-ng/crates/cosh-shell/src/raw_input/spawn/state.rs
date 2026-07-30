//! Relay thread state shared across the spawn submodules (#1721 layout
//! split): the per-relay bookkeeping struct and its borrow-splitting helper.

use std::fs::File;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::input::InputClassifier;

use super::super::capture_bridge::consume_captured_input;
use super::super::card_capture::CardInputState;
use super::super::event_parser::{CandidateLineBuffer, NativeLineState};
use super::super::generation::{LineSubmitCounter, UserPtyInputGeneration};
use super::super::mode::current_raw_input_mode;
use super::super::mode::RawInputMode;
use super::super::relay::{ExplicitExitTracker, InputRelayContext};
use super::super::{MainPromptGate, RawInputEvent};
use super::action::PendingDelayEscape;
use super::capture::{drain_capture_submission, CaptureOwnedInput};
use super::prompt_ghost::{PendingPromptGhostEscape, PendingReplacedPromptGhostSuffix};
use super::InputRead;

#[derive(Default)]
pub(in super::super) struct RawInputRelayState {
    pub(super) card_state: CardInputState,
    pub(super) line_buffer: CandidateLineBuffer,
    pub(super) native_line_state: NativeLineState,
    pub(super) exit_tracker: ExplicitExitTracker,
    pub(super) input_generation: UserPtyInputGeneration,
    pub(super) line_submits: LineSubmitCounter,
    pub(super) main_prompt_gate: MainPromptGate,
    /// Routes exact slash submissions through bash for native history
    /// recall (issue #1718); gated further by `main_prompt_gate` at
    /// submission time.
    pub(super) slash_route_enabled: bool,
    pub(super) pending_prompt_ghost_escape: Option<PendingPromptGhostEscape>,
    pub(super) pending_delay_escape: Option<PendingDelayEscape>,
    pub(super) pending_replaced_prompt_ghost_suffix: Option<PendingReplacedPromptGhostSuffix>,
    pub(super) capture_owned_input: CaptureOwnedInput,
    pub(super) deferred_input: Option<InputRead>,
    /// Deadline for a bare ESC held inside the draft card (#1721): on
    /// expiry the relay injects a second ESC which resolves to a cancel.
    pub(super) pending_draft_escape_deadline: Option<std::time::Instant>,
}

impl RawInputRelayState {
    pub(super) fn with_generation_and_gate(
        input_generation: UserPtyInputGeneration,
        main_prompt_gate: MainPromptGate,
        slash_route_enabled: bool,
    ) -> Self {
        Self {
            input_generation,
            main_prompt_gate,
            slash_route_enabled,
            ..Self::default()
        }
    }
}
pub(super) fn input_relay_context<'a>(
    master: &'a mut File,
    input_classifier: &'a InputClassifier,
    input_events: &'a Sender<RawInputEvent>,
    input_mode: &'a Arc<Mutex<RawInputMode>>,
    state: &'a mut RawInputRelayState,
) -> InputRelayContext<'a> {
    InputRelayContext {
        master,
        input_classifier,
        input_events,
        input_mode,
        input_generation: &state.input_generation,
        line_submits: &mut state.line_submits,
        line_buffer: &mut state.line_buffer,
        native_line_state: &mut state.native_line_state,
        exit_tracker: &mut state.exit_tracker,
        main_prompt_gate: &state.main_prompt_gate,
        slash_route_enabled: state.slash_route_enabled,
    }
}

// A bare ESC inside the draft card waits this long for a split CR/LF
// (legacy Alt+Enter) before it resolves to an explicit cancel (#1721).
const DRAFT_ESCAPE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(50);

/// Arms/disarms the draft-card ESC hold deadline from the card state; runs
/// once per relay loop turn before the next blocking receive (#1721).
pub(super) fn sync_pending_draft_escape(state: &mut RawInputRelayState) {
    if state.card_state.draft_escape_pending() {
        if state.pending_draft_escape_deadline.is_none() {
            state.pending_draft_escape_deadline = Some(Instant::now() + DRAFT_ESCAPE_TIMEOUT);
        }
    } else {
        state.pending_draft_escape_deadline = None;
    }
}

/// On expiry, injects a second ESC into the capture: combined with the held
/// first ESC it resolves to the explicit ESC+ESC cancel path, reusing the
/// normal release/mode bookkeeping (#1721). When the injected ESC releases
/// the capture, the Submitted -> Draining chain must drain here as well
/// (#1932): left pending, the quarantine would swallow the next keystroke
/// typed after the cancel (e.g. the first Up arrow).
pub(super) fn flush_pending_draft_escape(
    now: Instant,
    master: &mut File,
    input_classifier: &InputClassifier,
    input_events: &Sender<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> std::io::Result<()> {
    let Some(deadline) = state.pending_draft_escape_deadline else {
        return Ok(());
    };
    if now < deadline {
        return Ok(());
    }
    state.pending_draft_escape_deadline = None;
    let mode = current_raw_input_mode(input_mode);
    if let RawInputMode::Capture {
        capture,
        generation,
        ..
    } = mode
    {
        let result = consume_captured_input(
            &mut state.card_state,
            &capture,
            generation,
            b"\x1b",
            input_events,
            input_mode,
        );
        if result.generation.is_some() {
            let RawInputRelayState {
                card_state: _,
                line_buffer,
                native_line_state,
                exit_tracker,
                capture_owned_input,
                deferred_input,
                input_generation,
                line_submits,
                main_prompt_gate,
                slash_route_enabled,
                ..
            } = state;
            let mut relay = InputRelayContext {
                master,
                input_classifier,
                input_events,
                input_mode,
                input_generation,
                line_submits,
                line_buffer,
                native_line_state,
                exit_tracker,
                main_prompt_gate,
                slash_route_enabled: *slash_route_enabled,
            };
            relay.line_buffer.clear();
            relay.native_line_state.clear();
            drain_capture_submission(
                result,
                capture_owned_input,
                deferred_input,
                None,
                &mut relay,
            )?;
        }
    }
    Ok(())
}
