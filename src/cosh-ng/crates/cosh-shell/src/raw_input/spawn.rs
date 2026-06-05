use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use nix::libc;

use crate::input::InputClassifier;

use super::capture_bridge::consume_captured_input;
use super::card_capture::CardInputState;
use super::event_parser::{CandidateLineBuffer, NativeLineState};
use super::mode::{current_raw_input_mode, RawInputMode};
use super::pty::{set_pty_winsize, signal_process_group};
use super::relay::{
    relay_delayed_input, relay_passthrough_input, send_held_input_events, send_raw_input_events,
    ExplicitExitTracker, InputRelayContext,
};
use super::{RawInputEvent, RawRelayAction};

pub(crate) fn spawn_raw_input_relay<R>(
    mut input: R,
    mut master: File,
    input_events: Sender<RawInputEvent>,
    input_classifier: InputClassifier,
    input_mode: Arc<Mutex<RawInputMode>>,
) -> JoinHandle<io::Result<()>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut card_state = CardInputState::default();
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        loop {
            match input.read(&mut buffer) {
                Ok(0) => {
                    if !exit_tracker.saw_explicit_exit() {
                        master.write_all(b"exit\n")?;
                        master.flush()?;
                    }
                    return Ok(());
                }
                Ok(n) => match current_raw_input_mode(&input_mode) {
                    RawInputMode::Capture(capture) => {
                        if consume_captured_input(
                            &mut card_state,
                            &capture,
                            &buffer[..n],
                            &input_events,
                            &input_mode,
                        ) {
                            line_buffer.clear();
                            native_line_state.clear();
                        }
                    }
                    RawInputMode::Hold => {
                        card_state.reset();
                        send_held_input_events(&buffer[..n], &input_events);
                    }
                    RawInputMode::Delay => {
                        card_state.reset();
                        let mut relay = InputRelayContext {
                            master: &mut master,
                            input_classifier: &input_classifier,
                            input_events: &input_events,
                            input_mode: &input_mode,
                            line_buffer: &mut line_buffer,
                            native_line_state: &mut native_line_state,
                            exit_tracker: &mut exit_tracker,
                        };
                        relay_delayed_input(&buffer[..n], &mut relay)?;
                    }
                    RawInputMode::Passthrough => {
                        card_state.reset();
                        let mut relay = InputRelayContext {
                            master: &mut master,
                            input_classifier: &input_classifier,
                            input_events: &input_events,
                            input_mode: &input_mode,
                            line_buffer: &mut line_buffer,
                            native_line_state: &mut native_line_state,
                            exit_tracker: &mut exit_tracker,
                        };
                        if relay_passthrough_input(&buffer[..n], &mut relay)? {
                            continue;
                        }
                    }
                    RawInputMode::RawPassthrough => {
                        card_state.reset();
                        line_buffer.clear();
                        send_raw_input_events(&buffer[..n], &input_events);
                        native_line_state.observe_shell_bytes(&buffer[..n]);
                        exit_tracker.observe_shell_bytes(&buffer[..n]);
                        master.write_all(&buffer[..n])?;
                        master.flush()?;
                    }
                },
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    })
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
        (|| {
            let mut card_state = CardInputState::default();
            let mut line_buffer = CandidateLineBuffer::default();
            let mut native_line_state = NativeLineState::default();
            let mut exit_tracker = ExplicitExitTracker::default();
            for action in actions {
                match action {
                    RawRelayAction::Write(bytes) => match current_raw_input_mode(&input_mode) {
                        RawInputMode::Capture(capture) => {
                            if consume_captured_input(
                                &mut card_state,
                                &capture,
                                &bytes,
                                &input_events,
                                &input_mode,
                            ) {
                                line_buffer.clear();
                                native_line_state.clear();
                            }
                        }
                        RawInputMode::Hold => {
                            card_state.reset();
                            send_held_input_events(&bytes, &input_events);
                        }
                        RawInputMode::Delay => {
                            card_state.reset();
                            let mut relay = InputRelayContext {
                                master: &mut master,
                                input_classifier: &input_classifier,
                                input_events: &input_events,
                                input_mode: &input_mode,
                                line_buffer: &mut line_buffer,
                                native_line_state: &mut native_line_state,
                                exit_tracker: &mut exit_tracker,
                            };
                            relay_delayed_input(&bytes, &mut relay)?;
                        }
                        RawInputMode::Passthrough => {
                            card_state.reset();
                            let mut relay = InputRelayContext {
                                master: &mut master,
                                input_classifier: &input_classifier,
                                input_events: &input_events,
                                input_mode: &input_mode,
                                line_buffer: &mut line_buffer,
                                native_line_state: &mut native_line_state,
                                exit_tracker: &mut exit_tracker,
                            };
                            if relay_passthrough_input(&bytes, &mut relay)? {
                                continue;
                            }
                        }
                        RawInputMode::RawPassthrough => {
                            card_state.reset();
                            line_buffer.clear();
                            send_raw_input_events(&bytes, &input_events);
                            native_line_state.observe_shell_bytes(&bytes);
                            exit_tracker.observe_shell_bytes(&bytes);
                            master.write_all(&bytes)?;
                            master.flush()?;
                        }
                    },
                    RawRelayAction::Resize(winsize) => {
                        set_pty_winsize(master.as_raw_fd(), winsize)?;
                        signal_process_group(child_pid, libc::SIGWINCH)?;
                    }
                    RawRelayAction::Wait(duration) => thread::sleep(duration),
                }
            }

            if !exit_tracker.saw_explicit_exit() {
                master.write_all(b"exit\n")?;
                master.flush()?;
            }
            Ok(())
        })()
    })
}
