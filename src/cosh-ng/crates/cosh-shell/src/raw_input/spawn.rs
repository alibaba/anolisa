use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::libc;

use crate::input::InputClassifier;

use super::capture_bridge::consume_captured_input;
use super::card_capture::CardInputState;
use super::event_parser::{CandidateLineBuffer, NativeLineState};
use super::mode::{current_raw_input_mode, RawInputMode};
use super::pty::{set_pty_winsize, signal_process_group, write_all_pty};
use super::relay::{
    dismiss_prompt_ghost_input, relay_delayed_input, relay_passthrough_input,
    relay_prompt_ghost_input, send_held_input_events, send_raw_input_events, ExplicitExitTracker,
    InputRelayContext,
};
use super::{PromptGhostRoute, RawInputEvent, RawRelayAction};

const PROMPT_GHOST_ESCAPE_TIMEOUT: Duration = Duration::from_millis(50);
// Retain a complete split Shift+Tab sequence while the relay handles ESC.
const INPUT_READ_AHEAD_CAPACITY: usize = 3;

struct PendingPromptGhostEscape {
    bytes: Vec<u8>,
    text: String,
    route: PromptGhostRoute,
    deadline: Instant,
}

impl PendingPromptGhostEscape {
    fn matches_mode(&self, mode: &RawInputMode) -> bool {
        matches!(
            mode,
            RawInputMode::PromptGhost { text, route }
                if text == &self.text && route == &self.route
        )
    }
}

#[derive(Default)]
pub(super) struct RawInputRelayState {
    card_state: CardInputState,
    line_buffer: CandidateLineBuffer,
    native_line_state: NativeLineState,
    exit_tracker: ExplicitExitTracker,
    pending_prompt_ghost_escape: Option<PendingPromptGhostEscape>,
}

enum InputRead {
    Bytes {
        bytes: Vec<u8>,
        received_at: Instant,
    },
    Eof,
    Error(io::Error),
}

pub(crate) fn spawn_raw_input_relay<R>(
    input: R,
    mut master: File,
    input_events: Sender<RawInputEvent>,
    input_classifier: InputClassifier,
    input_mode: Arc<Mutex<RawInputMode>>,
) -> JoinHandle<io::Result<()>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let (read_tx, read_rx) = mpsc::sync_channel(INPUT_READ_AHEAD_CAPACITY);
        // The relay must wake without a later keystroke to resolve a bare ESC.
        thread::spawn(move || read_input_chunks(input, read_tx));

        let mut state = RawInputRelayState::default();
        loop {
            let input = match receive_input(&read_rx, &state) {
                Ok(input) => input,
                Err(RecvTimeoutError::Timeout) => {
                    flush_pending_prompt_ghost_escape(
                        false,
                        Instant::now(),
                        &mut master,
                        &input_events,
                        &input_classifier,
                        &input_mode,
                        &mut state,
                    )?;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => InputRead::Eof,
            };
            match input {
                InputRead::Bytes { bytes, received_at } => relay_input_bytes(
                    &bytes,
                    received_at,
                    &mut master,
                    &input_events,
                    &input_classifier,
                    &input_mode,
                    &mut state,
                )?,
                InputRead::Eof => {
                    finish_input_relay(
                        &mut master,
                        &input_events,
                        &input_classifier,
                        &input_mode,
                        &mut state,
                    )?;
                    return Ok(());
                }
                InputRead::Error(error) => return Err(error),
            }
        }
    })
}

fn read_input_chunks<R>(mut input: R, sender: SyncSender<InputRead>)
where
    R: Read,
{
    let mut buffer = [0_u8; 8192];
    loop {
        let input = match input.read(&mut buffer) {
            Ok(0) => InputRead::Eof,
            Ok(count) => InputRead::Bytes {
                bytes: buffer[..count].to_vec(),
                received_at: Instant::now(),
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => InputRead::Error(error),
        };
        let done = !matches!(input, InputRead::Bytes { .. });
        if sender.send(input).is_err() || done {
            return;
        }
    }
}

fn receive_input(
    receiver: &Receiver<InputRead>,
    state: &RawInputRelayState,
) -> Result<InputRead, RecvTimeoutError> {
    match state.pending_prompt_ghost_escape.as_ref() {
        Some(pending) => {
            receiver.recv_timeout(pending.deadline.saturating_duration_since(Instant::now()))
        }
        None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
    }
}

pub(super) fn relay_input_bytes(
    bytes: &[u8],
    received_at: Instant,
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> io::Result<()> {
    if state
        .pending_prompt_ghost_escape
        .as_ref()
        .is_some_and(|pending| received_at > pending.deadline)
    {
        flush_pending_prompt_ghost_escape(
            false,
            received_at,
            master,
            input_events,
            input_classifier,
            input_mode,
            state,
        )?;
    }

    let mode = current_raw_input_mode(input_mode);
    let mut pending_deadline = None;
    let mut combined = Vec::new();
    let bytes = if let Some(pending) = state.pending_prompt_ghost_escape.take() {
        if !pending.matches_mode(&mode) {
            // The pending prefix belongs to the previous ghost. Route it before
            // processing the new bytes so they cannot become its Shift+Tab suffix.
            relay_input_for_mode(
                &pending.bytes,
                mode,
                master,
                input_events,
                input_classifier,
                input_mode,
                state,
            )?;
            return relay_input_bytes(
                bytes,
                received_at,
                master,
                input_events,
                input_classifier,
                input_mode,
                state,
            );
        }
        pending_deadline = Some(pending.deadline);
        combined.extend_from_slice(&pending.bytes);
        combined.extend_from_slice(bytes);
        &combined
    } else {
        bytes
    };

    if let RawInputMode::PromptGhost {
        text,
        route: route @ PromptGhostRoute::AgentSelection { .. },
    } = &mode
    {
        if b"\x1b[Z".starts_with(bytes) && bytes.len() < 3 {
            state.pending_prompt_ghost_escape = Some(PendingPromptGhostEscape {
                bytes: bytes.to_vec(),
                text: text.clone(),
                route: route.clone(),
                deadline: pending_deadline.unwrap_or(received_at + PROMPT_GHOST_ESCAPE_TIMEOUT),
            });
            return Ok(());
        }
    }

    relay_input_for_mode(
        bytes,
        mode,
        master,
        input_events,
        input_classifier,
        input_mode,
        state,
    )
}

fn relay_input_for_mode(
    bytes: &[u8],
    mode: RawInputMode,
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> io::Result<()> {
    match mode {
        RawInputMode::Capture(capture) => {
            if consume_captured_input(
                &mut state.card_state,
                &capture,
                bytes,
                input_events,
                input_mode,
            ) {
                state.line_buffer.clear();
                state.native_line_state.clear();
            }
        }
        RawInputMode::Hold => {
            state.card_state.reset();
            send_held_input_events(bytes, input_events);
        }
        RawInputMode::Delay => {
            state.card_state.reset();
            let mut relay =
                input_relay_context(master, input_classifier, input_events, input_mode, state);
            relay_delayed_input(bytes, &mut relay)?;
        }
        RawInputMode::Passthrough => {
            state.card_state.reset();
            let mut relay =
                input_relay_context(master, input_classifier, input_events, input_mode, state);
            let _ = relay_passthrough_input(bytes, &mut relay)?;
        }
        RawInputMode::PromptGhost { text, route } => {
            state.card_state.reset();
            let mut relay =
                input_relay_context(master, input_classifier, input_events, input_mode, state);
            let _ = relay_prompt_ghost_input(bytes, &text, &route, &mut relay)?;
        }
        RawInputMode::RawPassthrough => {
            state.card_state.reset();
            state.line_buffer.clear();
            send_raw_input_events(bytes, input_events);
            state.native_line_state.observe_shell_bytes(bytes);
            state.exit_tracker.observe_shell_bytes(bytes);
            write_all_pty(master, bytes)?;
        }
    }
    Ok(())
}

fn input_relay_context<'a>(
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
        line_buffer: &mut state.line_buffer,
        native_line_state: &mut state.native_line_state,
        exit_tracker: &mut state.exit_tracker,
    }
}

fn flush_pending_prompt_ghost_escape(
    force: bool,
    now: Instant,
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> io::Result<()> {
    let should_flush = state
        .pending_prompt_ghost_escape
        .as_ref()
        .is_some_and(|pending| force || now >= pending.deadline);
    if !should_flush {
        return Ok(());
    }
    let Some(pending) = state.pending_prompt_ghost_escape.take() else {
        return Ok(());
    };
    let mode = current_raw_input_mode(input_mode);
    if pending.matches_mode(&mode) {
        let mut relay =
            input_relay_context(master, input_classifier, input_events, input_mode, state);
        let _ = dismiss_prompt_ghost_input(&pending.bytes, &mut relay)?;
        return Ok(());
    }
    relay_input_for_mode(
        &pending.bytes,
        mode,
        master,
        input_events,
        input_classifier,
        input_mode,
        state,
    )
}

pub(super) fn finish_input_relay(
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> io::Result<()> {
    flush_pending_prompt_ghost_escape(
        true,
        Instant::now(),
        master,
        input_events,
        input_classifier,
        input_mode,
        state,
    )?;
    if !state.exit_tracker.saw_explicit_exit() {
        write_all_pty(master, b"exit\n")?;
    }
    Ok(())
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
    while let Some(deadline) = state
        .pending_prompt_ghost_escape
        .as_ref()
        .map(|pending| pending.deadline)
    {
        if deadline > action_end {
            break;
        }
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
        // Scripted input must resolve ESC at the same boundary as live input.
        flush_pending_prompt_ghost_escape(
            false,
            Instant::now(),
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
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let mut state = RawInputRelayState::default();
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
