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
use super::{write_all_pty, PromptGhostRoute, RawInputEvent, RawInputMode, CTRL_C};

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

pub(super) fn send_shell_input_activity(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    if !bytes.is_empty() {
        let _ = input_events.send(RawInputEvent::ShellInputActivity);
    }
}

pub(super) fn relay_passthrough_input(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
) -> io::Result<bool> {
    relay_passthrough_input_with_activity(bytes, relay, true)
}

fn relay_passthrough_input_with_activity(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
    emit_activity: bool,
) -> io::Result<bool> {
    if relay.line_buffer.force_agent_intercept && relay.line_buffer.is_active() {
        relay.line_buffer.push(bytes);
        if !relay.line_buffer.force_agent_intercept {
            let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
            let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
            if !relay.line_buffer.is_active() {
                return Ok(true);
            }
            redraw_candidate_line(relay.input_events, relay.line_buffer);
            return relay_candidate_line(relay, emit_activity);
        }
        if !relay.line_buffer.is_active() {
            relay.line_buffer.clear();
            let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
            let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
            return Ok(true);
        }
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        return relay_candidate_line(relay, emit_activity);
    }
    if relay.input_classifier.is_conservative() {
        return relay_native_passthrough(bytes, relay, emit_activity);
    }
    if relay.line_buffer.is_active() || starts_intercept_candidate(bytes) {
        relay.line_buffer.push(bytes);
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        return relay_candidate_line(relay, emit_activity);
    }

    if emit_activity {
        send_shell_input_activity(bytes, relay.input_events);
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
    route: &PromptGhostRoute,
    relay: &mut InputRelayContext<'_>,
) -> io::Result<bool> {
    if bytes.starts_with(b"\t") && !relay.line_buffer.is_active() {
        let _ = relay.input_events.send(RawInputEvent::PromptGhostClear);
        let remainder = &bytes[1..];
        match route {
            PromptGhostRoute::NativeShell => {
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::RawPassthrough;
                }
                relay
                    .native_line_state
                    .observe_shell_bytes(ghost_text.as_bytes());
                relay
                    .exit_tracker
                    .observe_shell_bytes(ghost_text.as_bytes());
                write_all_pty(relay.master, ghost_text.as_bytes())?;
                if !remainder.is_empty() {
                    send_raw_input_events(remainder, relay.input_events);
                    relay.native_line_state.observe_shell_bytes(remainder);
                    relay.exit_tracker.observe_shell_bytes(remainder);
                    write_all_pty(relay.master, remainder)?;
                }
            }
            PromptGhostRoute::AgentIntercept { suggestion_id } => {
                relay.line_buffer.push(ghost_text.as_bytes());
                relay.line_buffer.force_agent_intercept = true;
                relay.line_buffer.forced_agent_suggestion_id = suggestion_id.clone();
                redraw_candidate_line(relay.input_events, relay.line_buffer);
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::Passthrough;
                }
                if !remainder.is_empty() {
                    relay_passthrough_input(remainder, relay)?;
                }
            }
        }
        return Ok(true);
    }
    if let Ok(mut mode) = relay.input_mode.lock() {
        *mode = RawInputMode::Passthrough;
    }
    let _ = relay.input_events.send(RawInputEvent::PromptGhostClear);
    let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
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
    if relay_passthrough_input_with_activity(bytes, relay, false)? {
        return Ok(());
    }
    Ok(())
}

fn relay_native_passthrough(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
    emit_activity: bool,
) -> io::Result<bool> {
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
                emit_activity,
            );
        }
        return relay_candidate_line(relay, emit_activity);
    }
    // Non-slash input: send directly to PTY. Shell marker's preexec/
    // command_not_found hooks handle NL/CJK intercept on the shell side.
    if emit_activity {
        send_shell_input_activity(bytes, relay.input_events);
    }
    send_raw_input_events(bytes, relay.input_events);
    relay.native_line_state.observe_shell_bytes(bytes);
    relay.exit_tracker.observe_shell_bytes(bytes);
    write_all_pty(relay.master, bytes)?;
    Ok(false)
}

fn relay_candidate_line(
    relay: &mut InputRelayContext<'_>,
    emit_activity: bool,
) -> io::Result<bool> {
    match candidate_line_status(&relay.line_buffer.bytes) {
        CandidateLineStatus::Pending => Ok(true),
        CandidateLineStatus::Unsafe if relay.line_buffer.force_agent_intercept => {
            relay.line_buffer.clear();
            let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
            let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
            Ok(true)
        }
        CandidateLineStatus::Unsafe => flush_candidate_line_to_shell(
            relay.master,
            relay.input_events,
            relay.line_buffer,
            relay.native_line_state,
            relay.exit_tracker,
            emit_activity,
        ),
        CandidateLineStatus::Complete { line, line_len } => {
            let force_agent_intercept = relay.line_buffer.force_agent_intercept;
            let suggestion_id = relay.line_buffer.forced_agent_suggestion_id.clone();
            let mut bytes = relay.line_buffer.take();
            let remainder = bytes.split_off(line_len);
            if force_agent_intercept {
                let _ = relay
                    .input_events
                    .send(RawInputEvent::CandidateCommit(line.as_bytes().to_vec()));
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::Delay;
                }
                let _ = relay
                    .input_events
                    .send(RawInputEvent::PromptGhostIntercept {
                        input: line,
                        suggestion_id,
                    });
                if !remainder.is_empty() {
                    relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
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
                        relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
                    }
                    Ok(true)
                }
                InputDecision::SendToShell(_) => {
                    let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
                    if emit_activity {
                        send_shell_input_activity(&bytes, relay.input_events);
                    }
                    send_raw_input_events(&bytes, relay.input_events);
                    relay.native_line_state.observe_shell_bytes(&bytes);
                    relay.exit_tracker.observe_shell_bytes(&bytes);
                    write_all_pty(relay.master, &bytes)?;
                    if !remainder.is_empty() {
                        relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
                    }
                    Ok(false)
                }
                InputDecision::Consume => {
                    let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
                    if !remainder.is_empty() {
                        relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
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
    emit_activity: bool,
) -> io::Result<bool> {
    let bytes = line_buffer.take();
    let _ = input_events.send(RawInputEvent::CandidateClearLine);
    if emit_activity {
        send_shell_input_activity(&bytes, input_events);
    }
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

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::sync::{mpsc, Arc, Mutex};

    use super::*;
    use crate::raw_input::PromptGhostRoute;

    fn output_file(label: &str) -> (std::path::PathBuf, File) {
        let path = std::env::temp_dir().join(format!(
            "cosh-shell-prompt-ghost-{label}-{}",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .expect("test output file");
        (path, file)
    }

    #[test]
    fn shell_rewrite_tab_writes_to_native_line_editor_without_agent_intercept() {
        let (path, mut master) = output_file("native");
        let (tx, rx) = mpsc::channel();
        let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
            text: "grep file".to_string(),
            route: PromptGhostRoute::NativeShell,
        }));
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let classifier = InputClassifier::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &tx,
            input_mode: &input_mode,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
        };

        assert!(relay_prompt_ghost_input(
            b"\t",
            "grep file",
            &PromptGhostRoute::NativeShell,
            &mut relay,
        )
        .expect("accept native ghost"));
        relay_passthrough_input(b"\t\x15", &mut relay)
            .expect("native completion and line clearing remain available");
        master.sync_all().expect("sync test output");

        assert_eq!(
            fs::read(&path).expect("read test output"),
            b"grep file\t\x15"
        );
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            vec![
                RawInputEvent::PromptGhostClear,
                RawInputEvent::ShellInputActivity
            ]
        );
        assert!(!line_buffer.force_agent_intercept);
        assert!(matches!(
            *input_mode.lock().expect("input mode"),
            RawInputMode::RawPassthrough
        ));
        fs::remove_file(path).ok();
    }

    #[test]
    fn agent_prompt_tab_stays_local_until_enter_and_keeps_suggestion_id() {
        let (path, mut master) = output_file("agent");
        let (tx, rx) = mpsc::channel();
        let route = PromptGhostRoute::AgentIntercept {
            suggestion_id: Some("suggestion-1".to_string()),
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
            text: "analyze failure".to_string(),
            route: route.clone(),
        }));
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let classifier = InputClassifier::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &tx,
            input_mode: &input_mode,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
        };

        relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
            .expect("accept agent ghost");
        assert!(rx
            .try_iter()
            .all(|event| !matches!(event, RawInputEvent::PromptGhostIntercept { .. })));

        relay_passthrough_input(b" safely\n", &mut relay).expect("submit edited agent prompt");
        assert!(rx.try_iter().any(|event| matches!(
            event,
            RawInputEvent::PromptGhostIntercept { input, suggestion_id }
                if input == "analyze failure safely"
                    && suggestion_id.as_deref() == Some("suggestion-1")
        )));
        assert_eq!(fs::read(&path).expect("read test output"), b"");
        fs::remove_file(path).ok();
    }

    #[test]
    fn clearing_accepted_agent_prompt_emits_binding_dismissal() {
        let (path, mut master) = output_file("clear-agent");
        let (tx, rx) = mpsc::channel();
        let route = PromptGhostRoute::AgentIntercept {
            suggestion_id: Some("suggestion-1".to_string()),
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
            text: "analyze failure".to_string(),
            route: route.clone(),
        }));
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let classifier = InputClassifier::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &tx,
            input_mode: &input_mode,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
        };

        relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
            .expect("accept agent ghost");
        relay_passthrough_input(&[0x15], &mut relay).expect("clear accepted prompt");

        assert!(rx
            .try_iter()
            .any(|event| event == RawInputEvent::PromptGhostDismissed));
        assert!(!line_buffer.is_active());
        fs::remove_file(path).ok();
    }

    #[test]
    fn unsupported_arrow_after_agent_prompt_tab_cancels_without_writing_to_shell() {
        let (path, mut master) = output_file("agent-arrow-cancel");
        let (tx, rx) = mpsc::channel();
        let route = PromptGhostRoute::AgentIntercept {
            suggestion_id: Some("suggestion-1".to_string()),
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
            text: "analyze failure".to_string(),
            route: route.clone(),
        }));
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let classifier = InputClassifier::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &tx,
            input_mode: &input_mode,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
        };

        relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
            .expect("accept agent ghost");
        relay_passthrough_input(b"\x1b[D", &mut relay).expect("cancel unsupported edit");
        master.sync_all().expect("sync test output");

        let events = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(fs::read(&path).expect("read test output"), b"");
        assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
        assert!(!events
            .iter()
            .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
        assert!(!line_buffer.is_active());
        assert!(line_buffer.forced_agent_suggestion_id.is_none());
        fs::remove_file(path).ok();
    }

    #[test]
    fn split_cursor_sequences_after_agent_prompt_tab_never_reach_shell() {
        for (name, sequence) in [
            ("left", b"\x1b[D".as_slice()),
            ("right", b"\x1b[C".as_slice()),
            ("home", b"\x1b[H".as_slice()),
            ("end", b"\x1b[F".as_slice()),
        ] {
            let (path, mut master) = output_file(&format!("agent-split-{name}"));
            let (tx, rx) = mpsc::channel();
            let route = PromptGhostRoute::AgentIntercept {
                suggestion_id: Some("suggestion-1".to_string()),
            };
            let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
                text: "analyze failure".to_string(),
                route: route.clone(),
            }));
            let mut line_buffer = CandidateLineBuffer::default();
            let mut native_line_state = NativeLineState::default();
            let mut exit_tracker = ExplicitExitTracker::default();
            let classifier = InputClassifier::default();
            let mut relay = InputRelayContext {
                master: &mut master,
                input_classifier: &classifier,
                input_events: &tx,
                input_mode: &input_mode,
                line_buffer: &mut line_buffer,
                native_line_state: &mut native_line_state,
                exit_tracker: &mut exit_tracker,
            };

            relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
                .expect("accept agent ghost");
            for byte in sequence {
                relay_passthrough_input(&[*byte], &mut relay).expect("relay split sequence");
            }
            master.sync_all().expect("sync test output");

            let events = rx.try_iter().collect::<Vec<_>>();
            assert_eq!(fs::read(&path).expect("read test output"), b"");
            assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
            assert!(!events
                .iter()
                .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
            assert!(!line_buffer.is_active());
            fs::remove_file(path).ok();
        }
    }

    #[test]
    fn clearing_and_submitting_in_one_buffer_dismisses_binding() {
        let (path, mut master) = output_file("clear-submit-agent");
        let (tx, rx) = mpsc::channel();
        let route = PromptGhostRoute::AgentIntercept {
            suggestion_id: Some("suggestion-1".to_string()),
        };
        let input_mode = Arc::new(Mutex::new(RawInputMode::PromptGhost {
            text: "analyze failure".to_string(),
            route: route.clone(),
        }));
        let mut line_buffer = CandidateLineBuffer::default();
        let mut native_line_state = NativeLineState::default();
        let mut exit_tracker = ExplicitExitTracker::default();
        let classifier = InputClassifier::default();
        let mut relay = InputRelayContext {
            master: &mut master,
            input_classifier: &classifier,
            input_events: &tx,
            input_mode: &input_mode,
            line_buffer: &mut line_buffer,
            native_line_state: &mut native_line_state,
            exit_tracker: &mut exit_tracker,
        };

        relay_prompt_ghost_input(b"\t", "analyze failure", &route, &mut relay)
            .expect("accept agent ghost");
        relay_passthrough_input(b"\x15\n", &mut relay).expect("clear and submit");

        let events = rx.try_iter().collect::<Vec<_>>();
        assert!(events.contains(&RawInputEvent::PromptGhostDismissed));
        assert!(!events
            .iter()
            .any(|event| matches!(event, RawInputEvent::PromptGhostIntercept { .. })));
        assert!(line_buffer.forced_agent_suggestion_id.is_none());
        assert_eq!(fs::read(&path).expect("read test output"), b"\n");
        fs::remove_file(path).ok();
    }
}
