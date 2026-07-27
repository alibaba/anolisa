use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::libc;

use crate::input::InputClassifier;

use super::super::generation::UserPtyInputGeneration;
use super::super::mode::RawInputMode;
use super::super::pty::{set_pty_winsize, signal_process_group};
use super::super::{RawInputEvent, RawRelayAction};
use super::{
    finish_input_relay, flush_pending_prompt_ghost_escape,
    flush_pending_replaced_prompt_ghost_suffix, next_pending_deadline, relay_input_bytes,
    relay_input_bytes_with_read_ahead, relay_input_for_mode, RawInputRelayState, RelayReadContext,
};

pub(super) struct PendingDelayEscape {
    pub(super) bytes: Vec<u8>,
    pub(super) deadline: Instant,
    pub(super) generation: u64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_pending_delay_escape(
    pending: PendingDelayEscape,
    bytes: &[u8],
    received_at: Instant,
    mode: &RawInputMode,
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
    read_context: RelayReadContext<'_>,
) -> io::Result<Option<Vec<u8>>> {
    if !matches!(
        mode,
        RawInputMode::Delay { generation } if *generation == pending.generation
    ) {
        if matches!(
            mode,
            RawInputMode::Passthrough | RawInputMode::RawPassthrough
        ) {
            relay_input_for_mode(
                &pending.bytes,
                mode.clone(),
                master,
                input_events,
                input_classifier,
                input_mode,
                state,
                RelayReadContext::default(),
            )?;
        }
        relay_input_bytes_with_read_ahead(
            bytes,
            received_at,
            master,
            input_events,
            input_classifier,
            input_mode,
            state,
            read_context,
        )?;
        return Ok(None);
    }
    if received_at > pending.deadline {
        let _ = input_events.send(RawInputEvent::Esc);
        relay_input_bytes_with_read_ahead(
            bytes,
            received_at,
            master,
            input_events,
            input_classifier,
            input_mode,
            state,
            read_context,
        )?;
        return Ok(None);
    }
    let mut combined = pending.bytes;
    combined.extend_from_slice(bytes);
    Ok(Some(combined))
}

pub(super) fn flush_pending_delay_escape(
    force: bool,
    now: Instant,
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> io::Result<()> {
    let should_flush = state
        .pending_delay_escape
        .as_ref()
        .is_some_and(|pending| force || now >= pending.deadline);
    if !should_flush {
        return Ok(());
    }
    let Some(pending) = state.pending_delay_escape.take() else {
        return Ok(());
    };
    let mode = super::super::mode::current_raw_input_mode(input_mode);
    match mode {
        RawInputMode::Delay { generation } if generation == pending.generation => {
            let _ = input_events.send(RawInputEvent::Esc);
            return Ok(());
        }
        RawInputMode::Passthrough | RawInputMode::RawPassthrough => {}
        _ => return Ok(()),
    }
    relay_input_for_mode(
        &pending.bytes,
        mode,
        master,
        input_events,
        input_classifier,
        input_mode,
        state,
        RelayReadContext::default(),
    )
}

fn wait_for_raw_action(
    duration: Duration,
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> io::Result<()> {
    let action_end = Instant::now() + duration;
    while let Some(deadline) = next_pending_deadline(state) {
        if deadline > action_end {
            break;
        }
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
        flush_pending_prompt_ghost_escape(
            false,
            Instant::now(),
            master,
            input_events,
            input_classifier,
            input_mode,
            state,
        )?;
        flush_pending_delay_escape(
            false,
            Instant::now(),
            master,
            input_events,
            input_classifier,
            input_mode,
            state,
        )?;
        let mode = super::super::mode::current_raw_input_mode(input_mode);
        flush_pending_replaced_prompt_ghost_suffix(
            false,
            Instant::now(),
            &mode,
            master,
            input_events,
            input_classifier,
            input_mode,
            state,
        )?;
    }
    thread::sleep(action_end.saturating_duration_since(Instant::now()));
    Ok(())
}

pub(crate) fn spawn_raw_action_relay(
    actions: Vec<RawRelayAction>,
    mut master: File,
    child_pid: u32,
    input_events: Sender<RawInputEvent>,
    input_classifier: InputClassifier,
    input_mode: Arc<Mutex<RawInputMode>>,
    input_generation: UserPtyInputGeneration,
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let mut state = RawInputRelayState::with_generation(input_generation);
        for action in actions {
            flush_pending_prompt_ghost_escape(
                false,
                Instant::now(),
                &mut master,
                &input_events,
                &input_classifier,
                &input_mode,
                &mut state,
            )?;
            flush_pending_delay_escape(
                false,
                Instant::now(),
                &mut master,
                &input_events,
                &input_classifier,
                &input_mode,
                &mut state,
            )?;
            let mode = super::super::mode::current_raw_input_mode(&input_mode);
            flush_pending_replaced_prompt_ghost_suffix(
                false,
                Instant::now(),
                &mode,
                &mut master,
                &input_events,
                &input_classifier,
                &input_mode,
                &mut state,
            )?;
            match action {
                RawRelayAction::Write(bytes) => relay_input_bytes(
                    &bytes,
                    Instant::now(),
                    &mut master,
                    &input_events,
                    &input_classifier,
                    &input_mode,
                    &mut state,
                )?,
                RawRelayAction::Resize(winsize) => {
                    set_pty_winsize(master.as_raw_fd(), winsize)?;
                    signal_process_group(child_pid, libc::SIGWINCH)?;
                }
                RawRelayAction::Wait(duration) => wait_for_raw_action(
                    duration,
                    &mut master,
                    &input_events,
                    &input_classifier,
                    &input_mode,
                    &mut state,
                )?,
            }
        }
        finish_input_relay(
            &mut master,
            &input_events,
            &input_classifier,
            &input_mode,
            &mut state,
        )
    })
}
