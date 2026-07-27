use super::*;

const CAPTURE_QUARANTINE_MAX_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct CaptureOwnedInput {
    bytes: usize,
    overflowed: bool,
}

impl CaptureOwnedInput {
    fn observe(&mut self, bytes: &[u8]) -> bool {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.overflowed || self.bytes <= CAPTURE_QUARANTINE_MAX_BYTES {
            return false;
        }
        self.overflowed = true;
        true
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

pub(super) fn capture_owns_input(mode: &RawInputMode) -> bool {
    matches!(
        mode,
        RawInputMode::Capture { .. }
            | RawInputMode::Submitted { .. }
            | RawInputMode::Draining { .. }
            | RawInputMode::Terminal { .. }
    )
}

pub(super) fn capture_generation(mode: &RawInputMode) -> Option<u64> {
    match mode {
        RawInputMode::Capture { generation, .. }
        | RawInputMode::Submitted { generation, .. }
        | RawInputMode::Draining { generation, .. } => Some(*generation),
        _ => None,
    }
}

pub(super) fn capture_quarantine_generation(
    observed_generation: Option<u64>,
    mode: &RawInputMode,
) -> Option<u64> {
    match observed_generation {
        Some(observed)
            if matches!(
                mode,
                RawInputMode::Capture { generation, .. } if *generation == observed
            ) =>
        {
            None
        }
        Some(observed) => Some(observed),
        None => None,
    }
}

pub(super) fn relay_input_chunk(
    bytes: &[u8],
    mut mode: RawInputMode,
    card_state: &mut CardInputState,
    capture_owned_input: &mut CaptureOwnedInput,
    deferred_input: &mut Option<InputRead>,
    read_ahead: Option<&Receiver<InputRead>>,
    expected_capture_generation: Option<u64>,
    relay: &mut InputRelayContext<'_>,
) -> io::Result<()> {
    loop {
        match mode {
            RawInputMode::Capture {
                capture,
                generation,
                ..
            } => {
                if let Some(expected_generation) = expected_capture_generation {
                    if expected_generation != generation {
                        relay_late_capture_bytes(
                            bytes,
                            expected_generation,
                            capture_owned_input,
                            relay,
                        )?;
                        return Ok(());
                    }
                }
                let result = consume_captured_input(
                    card_state,
                    &capture,
                    generation,
                    bytes,
                    relay.input_events,
                    relay.input_mode,
                );
                if result.retry {
                    if let Some(expected_generation) = expected_capture_generation {
                        relay_late_capture_bytes(
                            bytes,
                            expected_generation,
                            capture_owned_input,
                            relay,
                        )?;
                        return Ok(());
                    }
                    mode = current_raw_input_mode(relay.input_mode);
                    continue;
                }
                if result.generation.is_some() {
                    relay.line_buffer.clear();
                    relay.native_line_state.clear();
                    drain_capture_submission(
                        result,
                        capture_owned_input,
                        deferred_input,
                        read_ahead,
                        relay,
                    )?;
                }
                return Ok(());
            }
            RawInputMode::Submitted { .. } => {
                thread::sleep(Duration::from_millis(1));
                mode = current_raw_input_mode(relay.input_mode);
            }
            RawInputMode::Draining { .. } => {
                card_state.reset();
                drain_abandoned_capture(capture_owned_input, relay)?;
                mode = current_raw_input_mode(relay.input_mode);
            }
            RawInputMode::Hold => {
                card_state.reset();
                send_held_input_events(bytes, relay.input_events);
                return Ok(());
            }
            RawInputMode::Delay { .. } => {
                card_state.reset();
                relay_delayed_input(bytes, relay)?;
                return Ok(());
            }
            RawInputMode::Passthrough | RawInputMode::Terminal { .. } => {
                card_state.reset();
                relay_passthrough_input(bytes, relay)?;
                return Ok(());
            }
            RawInputMode::PromptGhost {
                text: ghost_text,
                route,
            } => {
                card_state.reset();
                relay_prompt_ghost_input(bytes, &ghost_text, &route, relay)?;
                return Ok(());
            }
            RawInputMode::RawPassthrough => {
                card_state.reset();
                relay.line_buffer.clear();
                send_raw_input_events(bytes, relay.input_events);
                relay.native_line_state.observe_shell_bytes(bytes);
                relay.exit_tracker.observe_shell_bytes(bytes);
                write_user_bytes_to_pty(
                    relay.master,
                    relay.input_generation,
                    relay.line_submits,
                    relay.input_events,
                    relay.main_prompt_gate,
                    bytes,
                )?;
                return Ok(());
            }
        }
    }
}

pub(super) fn drain_capture_submission(
    result: CaptureConsumeResult,
    capture_owned_input: &mut CaptureOwnedInput,
    deferred_input: &mut Option<InputRead>,
    read_ahead: Option<&Receiver<InputRead>>,
    relay: &mut InputRelayContext<'_>,
) -> io::Result<()> {
    let Some(generation) = result.generation else {
        return Ok(());
    };
    let mut overflow = capture_owned_input.observe(&result.remainder);
    if overflow {
        let _ = relay
            .input_events
            .send(RawInputEvent::CaptureOverflow { generation });
        expire_capture_submission(relay.input_mode, generation);
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match current_raw_input_mode(relay.input_mode) {
            RawInputMode::Draining {
                generation: active, ..
            } if active == generation => {
                if !overflow {
                    overflow = drain_capture_read_ahead(
                        generation,
                        capture_owned_input,
                        deferred_input,
                        read_ahead,
                        relay,
                    );
                }
                break;
            }
            RawInputMode::Submitted {
                generation: active, ..
            } if active == generation && Instant::now() < deadline => {
                let wait = deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(5));
                let Some(receiver) = read_ahead else {
                    thread::sleep(wait);
                    continue;
                };
                match receiver.recv_timeout(wait) {
                    Ok(InputRead::Bytes { bytes, .. }) => {
                        if !overflow && capture_owned_input.observe(&bytes) {
                            overflow = true;
                            let _ = relay
                                .input_events
                                .send(RawInputEvent::CaptureOverflow { generation });
                            expire_capture_submission(relay.input_mode, generation);
                        }
                    }
                    Ok(input @ (InputRead::Eof | InputRead::Error(_))) => {
                        *deferred_input = Some(input);
                        expire_capture_submission(relay.input_mode, generation);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        *deferred_input = Some(InputRead::Eof);
                        expire_capture_submission(relay.input_mode, generation);
                    }
                }
            }
            RawInputMode::Submitted {
                generation: active, ..
            } if active == generation => {
                let _ = relay
                    .input_events
                    .send(RawInputEvent::CaptureExpired { generation });
                expire_capture_submission(relay.input_mode, generation);
            }
            _ => return Ok(()),
        }
    }
    if !overflow && complete_capture_chain_if_pending(relay.input_mode, generation) {
        capture_owned_input.clear();
        let _ = relay
            .input_events
            .send(RawInputEvent::CaptureDrained { generation });
        return Ok(());
    }

    capture_owned_input.clear();
    complete_capture_replay(relay.input_mode, generation);
    let _ = relay
        .input_events
        .send(RawInputEvent::CaptureDrained { generation });
    Ok(())
}

pub(in super::super) fn relay_late_capture_input(
    bytes: &[u8],
    generation: u64,
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    input_classifier: &InputClassifier,
    input_mode: &Arc<Mutex<RawInputMode>>,
    state: &mut RawInputRelayState,
) -> io::Result<()> {
    let RawInputRelayState {
        line_buffer,
        native_line_state,
        exit_tracker,
        capture_owned_input,
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
    relay_late_capture_bytes(bytes, generation, capture_owned_input, &mut relay)
}

fn relay_late_capture_bytes(
    bytes: &[u8],
    generation: u64,
    capture_owned_input: &mut CaptureOwnedInput,
    relay: &mut InputRelayContext<'_>,
) -> io::Result<()> {
    let active_generation = match current_raw_input_mode(relay.input_mode) {
        RawInputMode::Capture {
            generation: active, ..
        }
        | RawInputMode::Submitted {
            generation: active, ..
        }
        | RawInputMode::Draining {
            generation: active, ..
        } => Some(active),
        _ => None,
    };
    if active_generation != Some(generation) {
        capture_owned_input.clear();
        return Ok(());
    }

    let overflow = capture_owned_input.observe(bytes);
    if overflow {
        let _ = relay
            .input_events
            .send(RawInputEvent::CaptureOverflow { generation });
        match current_raw_input_mode(relay.input_mode) {
            RawInputMode::Capture {
                generation: active, ..
            } if active == generation => abandon_active_capture(relay.input_mode),
            RawInputMode::Submitted {
                generation: active, ..
            }
            | RawInputMode::Draining {
                generation: active, ..
            } if active == generation => expire_capture_submission(relay.input_mode, active),
            _ => {}
        }
    }

    match current_raw_input_mode(relay.input_mode) {
        RawInputMode::Capture { .. } | RawInputMode::Submitted { .. } if !overflow => Ok(()),
        RawInputMode::Draining { .. } => drain_abandoned_capture(capture_owned_input, relay),
        _ => {
            capture_owned_input.clear();
            Ok(())
        }
    }
}

fn drain_capture_read_ahead(
    generation: u64,
    capture_owned_input: &mut CaptureOwnedInput,
    deferred_input: &mut Option<InputRead>,
    read_ahead: Option<&Receiver<InputRead>>,
    relay: &mut InputRelayContext<'_>,
) -> bool {
    let Some(receiver) = read_ahead else {
        return false;
    };
    loop {
        match receiver.try_recv() {
            Ok(InputRead::Bytes { bytes, .. }) => {
                if capture_owned_input.observe(&bytes) {
                    let _ = relay
                        .input_events
                        .send(RawInputEvent::CaptureOverflow { generation });
                    expire_capture_submission(relay.input_mode, generation);
                    return true;
                }
            }
            Ok(input @ (InputRead::Eof | InputRead::Error(_))) => {
                *deferred_input = Some(input);
                return false;
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => return false,
        }
    }
}

pub(super) fn drain_abandoned_capture(
    capture_owned_input: &mut CaptureOwnedInput,
    relay: &mut InputRelayContext<'_>,
) -> io::Result<()> {
    let RawInputMode::Draining { generation, .. } = current_raw_input_mode(relay.input_mode) else {
        return Ok(());
    };
    capture_owned_input.clear();
    complete_capture_replay(relay.input_mode, generation);
    let _ = relay
        .input_events
        .send(RawInputEvent::CaptureDrained { generation });
    Ok(())
}

pub(in super::super) fn finish_input_relay(
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
    super::action::flush_pending_delay_escape(
        true,
        Instant::now(),
        master,
        input_events,
        input_classifier,
        input_mode,
        state,
    )?;
    let mode = current_raw_input_mode(input_mode);
    flush_pending_replaced_prompt_ghost_suffix(
        true,
        Instant::now(),
        &mode,
        master,
        input_events,
        input_classifier,
        input_mode,
        state,
    )?;
    if let RawInputMode::Submitted { generation, .. } = current_raw_input_mode(input_mode) {
        expire_capture_submission(input_mode, generation);
    }
    abandon_active_capture(input_mode);
    if matches!(
        current_raw_input_mode(input_mode),
        RawInputMode::Draining { .. }
    ) {
        let RawInputRelayState {
            line_buffer,
            native_line_state,
            exit_tracker,
            capture_owned_input,
            input_generation,
            line_submits,
            main_prompt_gate,
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
            slash_route_enabled: false,
        };
        drain_abandoned_capture(capture_owned_input, &mut relay)?;
    }
    // Bytes held as a possible split paste delimiter never got a routing
    // verdict: forward them byte-identically before the trailing exit
    // (#1721; keeps partial CSI passthrough guarantees at EOF).
    let held_partial = state.line_buffer.take_pending_partial();
    if !held_partial.is_empty() {
        write_user_bytes_to_pty(
            master,
            &state.input_generation,
            &mut state.line_submits,
            input_events,
            &state.main_prompt_gate,
            &held_partial,
        )?;
    }
    if !state.exit_tracker.saw_explicit_exit() {
        write_user_bytes_to_pty(
            master,
            &state.input_generation,
            &mut state.line_submits,
            input_events,
            &state.main_prompt_gate,
            b"exit\n",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom};

    use super::*;
    use crate::raw_input::RawInputCapture;

    #[test]
    fn generation_cutoff_does_not_retry_input_into_the_replacement_capture() {
        let path = std::env::temp_dir().join(format!(
            "cosh-shell-capture-cutoff-retry-{}",
            std::process::id()
        ));
        let mut master = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .expect("test output file");
        let previous = RawInputCapture::Question {
            id: "q-1".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        };
        let next = RawInputCapture::Question {
            id: "q-2".to_string(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
            secret: false,
        };
        let stale_mode = RawInputMode::Capture {
            capture: previous.clone(),
            generation: 41,
            installed_at: Instant::now(),
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::Draining {
            previous_capture: previous.clone(),
            generation: 41,
            next_capture: Some(next.clone()),
            invalidated: false,
        }));
        let (input_tx, input_rx) = mpsc::channel();
        let classifier = InputClassifier::default();
        let mut card_state = CardInputState::default();
        let mut quarantine = CaptureOwnedInput::default();
        let mut deferred_input = None;
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let input_generation = UserPtyInputGeneration::default();
        let mut line_submits = LineSubmitCounter::default();
        let main_prompt_gate = super::super::super::MainPromptGate::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &input_tx,
            input_mode: &input_mode,
            input_generation: &input_generation,
            line_submits: &mut line_submits,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
            main_prompt_gate: &main_prompt_gate,
            slash_route_enabled: false,
        };

        relay_input_chunk(
            b"stale",
            stale_mode,
            &mut card_state,
            &mut quarantine,
            &mut deferred_input,
            None,
            Some(41),
            &mut relay,
        )
        .expect("relay stale input");

        assert!(!input_rx
            .try_iter()
            .any(|event| matches!(event, RawInputEvent::CardInput(target, _) if target == "q-2")));
        master.sync_all().expect("sync test output");
        assert!(fs::read(&path).expect("read test output").is_empty());
        assert!(matches!(
            current_raw_input_mode(&input_mode),
            RawInputMode::Capture {
                capture: RawInputCapture::Question { id, .. },
                ..
            } if id == "q-2"
        ));

        master.set_len(0).expect("truncate test output");
        master.seek(SeekFrom::Start(0)).expect("rewind test output");
        *input_mode.lock().expect("input mode") = RawInputMode::Draining {
            previous_capture: previous,
            generation: 41,
            next_capture: Some(next),
            invalidated: false,
        };
        let draining_snapshot = current_raw_input_mode(&input_mode);
        let mut card_state = CardInputState::default();
        let mut quarantine = CaptureOwnedInput::default();
        let mut deferred_input = None;
        let mut line_submits = LineSubmitCounter::default();
        let main_prompt_gate = super::super::super::MainPromptGate::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &input_tx,
            input_mode: &input_mode,
            input_generation: &input_generation,
            line_submits: &mut line_submits,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
            main_prompt_gate: &main_prompt_gate,
            slash_route_enabled: false,
        };
        relay_input_chunk(
            b"later",
            draining_snapshot,
            &mut card_state,
            &mut quarantine,
            &mut deferred_input,
            None,
            Some(41),
            &mut relay,
        )
        .expect("relay input across draining snapshot");
        abandon_active_capture(&input_mode);
        drain_abandoned_capture(&mut quarantine, &mut relay).expect("drain replacement capture");

        assert!(!input_rx
            .try_iter()
            .any(|event| matches!(event, RawInputEvent::CardInput(target, _) if target == "q-2")));
        master.sync_all().expect("sync test output");
        assert!(fs::read(&path).expect("read test output").is_empty());
        fs::remove_file(path).ok();
    }
}
