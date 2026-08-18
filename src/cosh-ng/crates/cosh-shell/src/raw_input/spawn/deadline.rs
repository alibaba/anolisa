//! Blocking receive deadlines for split escape-sequence handling.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use super::{InputRead, RawInputRelayState};

pub(super) fn receive_input(
    receiver: &Receiver<InputRead>,
    state: &mut RawInputRelayState,
) -> Result<InputRead, RecvTimeoutError> {
    if let Some(input) = state.deferred_input.take() {
        return Ok(input);
    }
    match next_pending_deadline(state) {
        Some(deadline) => receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
        None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
    }
}

pub(super) fn next_pending_deadline(state: &RawInputRelayState) -> Option<Instant> {
    state
        .pending_prompt_ghost_escape
        .as_ref()
        .map(|pending| pending.deadline)
        .into_iter()
        .chain(
            state
                .pending_delay_escape
                .as_ref()
                .map(|pending| pending.deadline),
        )
        .chain(
            state
                .pending_replaced_prompt_ghost_suffix
                .as_ref()
                .map(|pending| pending.deadline),
        )
        .chain(state.pending_draft_escape_deadline)
        .min()
}
