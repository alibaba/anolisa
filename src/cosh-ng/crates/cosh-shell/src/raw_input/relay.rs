use std::fs::File;
use std::io;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::input::{InputClassifier, InputDecision, InterceptReason};

use super::event_parser::{
    candidate_inline_hint, candidate_line_status, native_candidate_should_return_to_shell,
    starts_intercept_candidate, starts_native_intercept_candidate, CandidateLineBuffer,
    CandidateLineStatus, NativeLineState,
};
use super::{write_all_pty, RawInputEvent, RawInputMode, CTRL_C};

pub(super) struct InputRelayContext<'a> {
    pub(super) master: &'a mut File,
    pub(super) input_classifier: &'a InputClassifier,
    pub(super) input_events: &'a Sender<RawInputEvent>,
    pub(super) input_mode: &'a Arc<Mutex<RawInputMode>>,
    pub(super) line_buffer: &'a mut CandidateLineBuffer,
    pub(super) native_line_state: &'a mut NativeLineState,
    pub(super) exit_tracker: &'a mut ExplicitExitTracker,
}

pub(super) fn send_raw_input_events(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    if bytes.contains(&CTRL_C) {
        let _ = input_events.send(RawInputEvent::CtrlC);
    }
}

pub(super) fn relay_passthrough_input(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
) -> io::Result<bool> {
    if relay.input_classifier.is_conservative() {
        return relay_native_passthrough(bytes, relay);
    }
    if relay.line_buffer.is_active() || starts_intercept_candidate(bytes) {
        relay.line_buffer.push(bytes);
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        return relay_candidate_line(relay);
    }

    send_raw_input_events(bytes, relay.input_events);
    relay.native_line_state.observe_shell_bytes(bytes);
    relay.exit_tracker.observe_shell_bytes(bytes);
    write_all_pty(relay.master, bytes)?;
    Ok(false)
}

pub(super) fn relay_prompt_ghost_input(
    bytes: &[u8],
    ghost_text: &str,
    relay: &mut InputRelayContext<'_>,
) -> io::Result<bool> {
    if bytes.starts_with(b"\t") && !relay.line_buffer.is_active() {
        let _ = relay.input_events.send(RawInputEvent::PromptGhostClear);
        relay.line_buffer.push(ghost_text.as_bytes());
        relay.line_buffer.force_agent_intercept = true;
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        if let Ok(mut mode) = relay.input_mode.lock() {
            *mode = RawInputMode::Passthrough;
        }
        let remainder = &bytes[1..];
        if !remainder.is_empty() {
            relay_passthrough_input(remainder, relay)?;
        }
        return Ok(true);
    }
    if let Ok(mut mode) = relay.input_mode.lock() {
        *mode = RawInputMode::Passthrough;
    }
    let _ = relay.input_events.send(RawInputEvent::PromptGhostClear);
    relay_passthrough_input(bytes, relay)
}

pub(super) fn send_held_input_events(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    send_raw_input_events(bytes, input_events);
    if held_input_requests_cancel(bytes) {
        let _ = input_events.send(RawInputEvent::CtrlC);
    }
}

pub(super) fn relay_delayed_input(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
) -> io::Result<()> {
    if bytes.contains(&CTRL_C) {
        let _ = relay.input_events.send(RawInputEvent::CtrlC);
        relay.line_buffer.clear();
        relay.native_line_state.clear();
        return Ok(());
    }
    if relay_passthrough_input(bytes, relay)? {
        return Ok(());
    }
    Ok(())
}

fn relay_native_passthrough(bytes: &[u8], relay: &mut InputRelayContext<'_>) -> io::Result<bool> {
    if relay.line_buffer.is_active()
        || starts_native_intercept_candidate(bytes, relay.native_line_state)
    {
        relay.line_buffer.push(bytes);
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        if native_candidate_should_return_to_shell(relay.input_classifier, relay.line_buffer) {
            return flush_candidate_line_to_shell(
                relay.master,
                relay.input_events,
                relay.line_buffer,
                relay.native_line_state,
                relay.exit_tracker,
            );
        }
        return relay_candidate_line(relay);
    }
    // Non-slash input: send directly to PTY. Shell marker's preexec/
    // command_not_found hooks handle NL/CJK intercept on the shell side.
    send_raw_input_events(bytes, relay.input_events);
    relay.native_line_state.observe_shell_bytes(bytes);
    relay.exit_tracker.observe_shell_bytes(bytes);
    write_all_pty(relay.master, bytes)?;
    Ok(false)
}

fn relay_candidate_line(relay: &mut InputRelayContext<'_>) -> io::Result<bool> {
    match candidate_line_status(&relay.line_buffer.bytes) {
        CandidateLineStatus::Pending => Ok(true),
        CandidateLineStatus::Unsafe => flush_candidate_line_to_shell(
            relay.master,
            relay.input_events,
            relay.line_buffer,
            relay.native_line_state,
            relay.exit_tracker,
        ),
        CandidateLineStatus::Complete { line, line_len } => {
            let force_agent_intercept = relay.line_buffer.force_agent_intercept;
            let mut bytes = relay.line_buffer.take();
            let remainder = bytes.split_off(line_len);
            if force_agent_intercept {
                let _ = relay
                    .input_events
                    .send(RawInputEvent::CandidateCommit(line.as_bytes().to_vec()));
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::Delay;
                }
                let _ = relay.input_events.send(RawInputEvent::UserIntercept(
                    line,
                    InterceptReason::NaturalLanguage,
                ));
                if !remainder.is_empty() {
                    relay_passthrough_input(&remainder, relay)?;
                }
                return Ok(true);
            }
            match relay.input_classifier.classify(&line) {
                InputDecision::Intercept { input, reason } => {
                    let _ = relay
                        .input_events
                        .send(RawInputEvent::CandidateCommit(line.as_bytes().to_vec()));
                    if let Ok(mut mode) = relay.input_mode.lock() {
                        *mode = RawInputMode::Delay;
                    }
                    let _ = relay
                        .input_events
                        .send(RawInputEvent::UserIntercept(input, reason));
                    if !remainder.is_empty() {
                        relay_passthrough_input(&remainder, relay)?;
                    }
                    Ok(true)
                }
                InputDecision::SendToShell(_) => {
                    let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
                    send_raw_input_events(&bytes, relay.input_events);
                    relay.native_line_state.observe_shell_bytes(&bytes);
                    relay.exit_tracker.observe_shell_bytes(&bytes);
                    write_all_pty(relay.master, &bytes)?;
                    if !remainder.is_empty() {
                        relay_passthrough_input(&remainder, relay)?;
                    }
                    Ok(false)
                }
                InputDecision::Consume => {
                    let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
                    if !remainder.is_empty() {
                        relay_passthrough_input(&remainder, relay)?;
                    }
                    Ok(false)
                }
            }
        }
    }
}

fn flush_candidate_line_to_shell(
    master: &mut File,
    input_events: &Sender<RawInputEvent>,
    line_buffer: &mut CandidateLineBuffer,
    native_line_state: &mut NativeLineState,
    exit_tracker: &mut ExplicitExitTracker,
) -> io::Result<bool> {
    let bytes = line_buffer.take();
    let _ = input_events.send(RawInputEvent::CandidateClearLine);
    send_raw_input_events(&bytes, input_events);
    native_line_state.observe_shell_bytes(&bytes);
    exit_tracker.observe_shell_bytes(&bytes);
    write_all_pty(master, &bytes)?;
    Ok(false)
}

fn redraw_candidate_line(
    input_events: &Sender<RawInputEvent>,
    line_buffer: &mut CandidateLineBuffer,
) {
    let visible = line_buffer.visible_line_bytes().to_vec();
    let hint = std::str::from_utf8(&visible)
        .ok()
        .and_then(candidate_inline_hint);
    line_buffer.relayed_len = visible.len();
    let _ = input_events.send(RawInputEvent::CandidateRedraw {
        input: visible,
        hint,
    });
}

fn held_input_requests_cancel(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes)
        .lines()
        .any(|line| line.split_whitespace().next() == Some("/cancel"))
}

#[derive(Debug, Default)]
pub(super) struct ExplicitExitTracker {
    pending_line: Vec<u8>,
    saw_explicit_exit: bool,
}

impl ExplicitExitTracker {
    pub(super) fn observe_shell_bytes(&mut self, bytes: &[u8]) {
        if self.saw_explicit_exit {
            return;
        }
        self.pending_line.extend_from_slice(bytes);
        while let Some(idx) = self
            .pending_line
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
        {
            let line = self.pending_line.drain(..=idx).collect::<Vec<_>>();
            if is_explicit_exit_line(&line) {
                self.saw_explicit_exit = true;
                self.pending_line.clear();
                return;
            }
        }
        if self.pending_line.len() > 4096 {
            self.pending_line.clear();
        }
    }

    pub(super) fn saw_explicit_exit(&self) -> bool {
        self.saw_explicit_exit
    }
}

fn is_explicit_exit_line(line: &[u8]) -> bool {
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim();
    trimmed == "exit" || trimmed.starts_with("exit ") || trimmed == "logout"
}
