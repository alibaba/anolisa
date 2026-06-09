use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nix::libc;
use nix::pty::Winsize;

use crate::input::{InputClassifier, InputDecision, InterceptReason};

mod card_capture;

use card_capture::CardInputState;

const CTRL_C: u8 = 0x03;

#[derive(Debug, Clone)]
pub enum RawRelayAction {
    Write(Vec<u8>),
    Resize(Winsize),
    Wait(Duration),
}

impl RawRelayAction {
    pub fn write(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Write(bytes.into())
    }

    pub fn line(line: impl AsRef<str>) -> Self {
        let mut bytes = line.as_ref().as_bytes().to_vec();
        bytes.push(b'\n');
        Self::Write(bytes)
    }

    pub fn resize(rows: u16, cols: u16) -> Self {
        Self::Resize(Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        })
    }

    pub fn wait(duration: Duration) -> Self {
        Self::Wait(duration)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawInputEvent {
    CtrlC,
    CandidateRedraw {
        input: Vec<u8>,
        hint: Option<String>,
    },
    CandidateCommit(Vec<u8>),
    CandidateClearLine,
    UserIntercept(String, InterceptReason),
    CardFocus(String, usize),
    CardToggle(String, usize),
    CardInput(String, String),
    CardApprove(String),
    CardDeny(String),
    CardDetails(String),
    CardCancel(String),
    CardAnswer(String),
    ModeFocus(String, usize),
    ModeSet(String, usize),
    ModeCancel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawObserverAction {
    Continue,
    RawPassthrough,
    HoldShellOutput,
    DelayShellOutput,
    CaptureInput(RawInputCapture),
    EmitToPty(Vec<u8>),
    RestorePrompt,
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
    Delay,
    Capture(RawInputCapture),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawInputCapture {
    Question {
        id: String,
        option_count: usize,
        allow_free_text: bool,
        multiple: bool,
    },
    Approval {
        id: String,
    },
    Mode {
        id: String,
        option_count: usize,
        selected: usize,
    },
    Consultation {
        id: String,
    },
}

pub(crate) fn update_input_mode(input_mode: &Arc<Mutex<RawInputMode>>, action: &RawObserverAction) {
    let Ok(mut mode) = input_mode.lock() else {
        return;
    };
    *mode = match action {
        RawObserverAction::CaptureInput(capture) => RawInputMode::Capture(capture.clone()),
        RawObserverAction::HoldShellOutput => RawInputMode::Hold,
        RawObserverAction::DelayShellOutput => RawInputMode::Delay,
        RawObserverAction::RawPassthrough => RawInputMode::RawPassthrough,
        RawObserverAction::Continue
        | RawObserverAction::EmitToPty(_)
        | RawObserverAction::RestorePrompt => RawInputMode::Passthrough,
    };
}

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
                        card_state.apply_capture(&capture);
                        let events = card_state.consume(&capture, &buffer[..n]);
                        if events.iter().any(releases_mode_capture) {
                            if let Ok(mut mode) = input_mode.lock() {
                                *mode = RawInputMode::Passthrough;
                            }
                            card_state.reset();
                        }
                        for event in events {
                            let _ = input_events.send(event);
                        }
                    }
                    RawInputMode::Hold => {
                        card_state.reset();
                        send_held_input_events(&buffer[..n], &input_events);
                    }
                    RawInputMode::Delay => {
                        card_state.reset();
                        relay_delayed_input(
                            &buffer[..n],
                            &mut master,
                            &input_classifier,
                            &input_events,
                            &input_mode,
                            &mut line_buffer,
                            &mut native_line_state,
                            &mut exit_tracker,
                        )?;
                    }
                    RawInputMode::Passthrough => {
                        card_state.reset();
                        if relay_passthrough_input(
                            &buffer[..n],
                            &mut master,
                            &input_classifier,
                            &input_events,
                            &input_mode,
                            &mut line_buffer,
                            &mut native_line_state,
                            &mut exit_tracker,
                        )? {
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

fn current_raw_input_mode(input_mode: &Arc<Mutex<RawInputMode>>) -> RawInputMode {
    input_mode
        .lock()
        .map(|mode| mode.clone())
        .unwrap_or(RawInputMode::Passthrough)
}

fn send_raw_input_events(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    if bytes.contains(&CTRL_C) {
        let _ = input_events.send(RawInputEvent::CtrlC);
    }
}

#[derive(Debug, Default)]
struct CandidateLineBuffer {
    bytes: Vec<u8>,
    relayed_len: usize,
}

impl CandidateLineBuffer {
    fn is_active(&self) -> bool {
        !self.bytes.is_empty()
    }

    fn push(&mut self, bytes: &[u8]) {
        let mut idx = 0;
        while idx < bytes.len() {
            match bytes[idx] {
                0x7f | 0x08 => {
                    self.pop_visible_char();
                    idx += 1;
                }
                0x1b if bytes.get(idx + 1) == Some(&b'[')
                    && bytes.get(idx + 2) == Some(&b'3')
                    && bytes.get(idx + 3) == Some(&b'~') =>
                {
                    self.pop_visible_char();
                    idx += 4;
                }
                byte => {
                    self.bytes.push(byte);
                    idx += 1;
                }
            }
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.relayed_len = 0;
    }

    fn take(&mut self) -> Vec<u8> {
        self.relayed_len = 0;
        std::mem::take(&mut self.bytes)
    }

    fn visible_line_bytes(&self) -> &[u8] {
        let end = self
            .bytes
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
            .unwrap_or(self.bytes.len());
        &self.bytes[..end]
    }

    fn pop_visible_char(&mut self) {
        let Some(end) = self
            .bytes
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
            .or(Some(self.bytes.len()))
        else {
            return;
        };
        if end == 0 {
            return;
        }
        let mut start = end - 1;
        while start > 0 && (self.bytes[start] & 0b1100_0000) == 0b1000_0000 {
            start -= 1;
        }
        self.bytes.drain(start..end);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CandidateLineStatus {
    Pending,
    Complete { line: String, line_len: usize },
    Unsafe,
}

#[derive(Debug, Default)]
struct NativeLineState {
    visible: Vec<u8>,
}

impl NativeLineState {
    fn is_at_line_start(&self) -> bool {
        self.visible.is_empty()
    }

    fn observe_shell_bytes(&mut self, bytes: &[u8]) {
        let mut idx = 0;
        while idx < bytes.len() {
            match bytes[idx] {
                CTRL_C | b'\n' | b'\r' => {
                    self.clear();
                    idx += 1;
                }
                0x7f | 0x08 => {
                    self.pop_visible_char();
                    idx += 1;
                }
                0x1b if bytes.get(idx + 1) == Some(&b'[')
                    && bytes.get(idx + 2) == Some(&b'3')
                    && bytes.get(idx + 3) == Some(&b'~') =>
                {
                    self.pop_visible_char();
                    idx += 4;
                }
                b'\t' => {
                    idx += 1;
                }
                byte if byte < 0x20 || byte == 0x1b => {
                    idx += 1;
                }
                byte => {
                    self.visible.push(byte);
                    idx += 1;
                }
            }
        }
        if self.visible.len() > 4096 {
            self.clear();
        }
    }

    fn clear(&mut self) {
        self.visible.clear();
    }

    fn pop_visible_char(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let mut start = self.visible.len() - 1;
        while start > 0 && (self.visible[start] & 0b1100_0000) == 0b1000_0000 {
            start -= 1;
        }
        self.visible.drain(start..);
    }
}

fn relay_passthrough_input(
    bytes: &[u8],
    master: &mut File,
    input_classifier: &InputClassifier,
    input_events: &Sender<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
    line_buffer: &mut CandidateLineBuffer,
    native_line_state: &mut NativeLineState,
    exit_tracker: &mut ExplicitExitTracker,
) -> io::Result<bool> {
    if input_classifier.is_conservative() {
        return relay_native_passthrough(
            bytes,
            master,
            input_classifier,
            input_events,
            input_mode,
            line_buffer,
            native_line_state,
            exit_tracker,
        );
    }
    if line_buffer.is_active() || starts_intercept_candidate(bytes) {
        line_buffer.push(bytes);
        redraw_candidate_line(input_events, line_buffer);
        return relay_candidate_line(
            master,
            input_classifier,
            input_events,
            input_mode,
            line_buffer,
            native_line_state,
            exit_tracker,
        );
    }

    send_raw_input_events(bytes, input_events);
    native_line_state.observe_shell_bytes(bytes);
    exit_tracker.observe_shell_bytes(bytes);
    master.write_all(bytes)?;
    master.flush()?;
    Ok(false)
}

fn relay_native_passthrough(
    bytes: &[u8],
    master: &mut File,
    input_classifier: &InputClassifier,
    input_events: &Sender<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
    line_buffer: &mut CandidateLineBuffer,
    native_line_state: &mut NativeLineState,
    exit_tracker: &mut ExplicitExitTracker,
) -> io::Result<bool> {
    if line_buffer.is_active() || starts_native_intercept_candidate(bytes, native_line_state) {
        line_buffer.push(bytes);
        redraw_candidate_line(input_events, line_buffer);
        if native_candidate_should_return_to_shell(input_classifier, line_buffer) {
            return flush_candidate_line_to_shell(
                master,
                input_events,
                line_buffer,
                native_line_state,
                exit_tracker,
            );
        }
        return relay_candidate_line(
            master,
            input_classifier,
            input_events,
            input_mode,
            line_buffer,
            native_line_state,
            exit_tracker,
        );
    }
    // Non-slash input: send directly to PTY. Shell marker's preexec/
    // command_not_found hooks handle NL/CJK intercept on the shell side.
    send_raw_input_events(bytes, input_events);
    native_line_state.observe_shell_bytes(bytes);
    exit_tracker.observe_shell_bytes(bytes);
    master.write_all(bytes)?;
    master.flush()?;
    Ok(false)
}

fn relay_candidate_line(
    master: &mut File,
    input_classifier: &InputClassifier,
    input_events: &Sender<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
    line_buffer: &mut CandidateLineBuffer,
    native_line_state: &mut NativeLineState,
    exit_tracker: &mut ExplicitExitTracker,
) -> io::Result<bool> {
    match candidate_line_status(&line_buffer.bytes) {
        CandidateLineStatus::Pending => Ok(true),
        CandidateLineStatus::Unsafe => flush_candidate_line_to_shell(
            master,
            input_events,
            line_buffer,
            native_line_state,
            exit_tracker,
        ),
        CandidateLineStatus::Complete { line, line_len } => {
            let mut bytes = line_buffer.take();
            let remainder = bytes.split_off(line_len);
            match input_classifier.classify(&line) {
                InputDecision::Intercept { input, reason } => {
                    let _ =
                        input_events.send(RawInputEvent::CandidateCommit(line.as_bytes().to_vec()));
                    if let Ok(mut mode) = input_mode.lock() {
                        *mode = RawInputMode::Delay;
                    }
                    let _ = input_events.send(RawInputEvent::UserIntercept(input, reason));
                    if !remainder.is_empty() {
                        relay_passthrough_input(
                            &remainder,
                            master,
                            input_classifier,
                            input_events,
                            input_mode,
                            line_buffer,
                            native_line_state,
                            exit_tracker,
                        )?;
                    }
                    Ok(true)
                }
                InputDecision::SendToShell(_) => {
                    let _ = input_events.send(RawInputEvent::CandidateClearLine);
                    send_raw_input_events(&bytes, input_events);
                    native_line_state.observe_shell_bytes(&bytes);
                    exit_tracker.observe_shell_bytes(&bytes);
                    master.write_all(&bytes)?;
                    master.flush()?;
                    if !remainder.is_empty() {
                        relay_passthrough_input(
                            &remainder,
                            master,
                            input_classifier,
                            input_events,
                            input_mode,
                            line_buffer,
                            native_line_state,
                            exit_tracker,
                        )?;
                    }
                    Ok(false)
                }
                InputDecision::Consume => {
                    let _ = input_events.send(RawInputEvent::CandidateClearLine);
                    if !remainder.is_empty() {
                        relay_passthrough_input(
                            &remainder,
                            master,
                            input_classifier,
                            input_events,
                            input_mode,
                            line_buffer,
                            native_line_state,
                            exit_tracker,
                        )?;
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
    master.write_all(&bytes)?;
    master.flush()?;
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

fn candidate_inline_hint(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('/') || trimmed[1..].contains('/') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let token = parts.next().unwrap_or_default();
    match token {
        "/" => None,
        "/m" | "/mo" | "/mod" => Some("/mode [recommend|agent]".to_string()),
        "/mode" | "/approval-mode" if parts.next().is_none() => {
            Some("[recommend|agent]".to_string())
        }
        "/d" | "/de" | "/det" | "/deta" | "/detai" | "/detail" => Some("/details <id>".to_string()),
        "/details" if parts.next().is_none() => Some("<id>".to_string()),
        "/h" | "/he" | "/hel" => Some("/help".to_string()),
        "/s" | "/sk" | "/ski" | "/skil" => Some("/skill".to_string()),
        "/c" | "/co" | "/con" | "/conf" | "/confi" => Some("/config".to_string()),
        "/a" | "/au" | "/aud" | "/audi" => Some("/audit".to_string()),
        _ => None,
    }
}

fn starts_intercept_candidate(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(b'/' | b'?')) || bytes.first().is_some_and(|byte| *byte >= 0x80)
}

fn starts_native_intercept_candidate(bytes: &[u8], native_line_state: &NativeLineState) -> bool {
    native_line_state.is_at_line_start()
        && (bytes.first() == Some(&b'/') || bytes.starts_with(b"??"))
}

fn native_candidate_should_return_to_shell(
    input_classifier: &InputClassifier,
    line_buffer: &CandidateLineBuffer,
) -> bool {
    let visible = line_buffer.visible_line_bytes();
    if visible.contains(&b'\t') {
        return true;
    }
    let Ok(line) = std::str::from_utf8(visible) else {
        return false;
    };
    let token = line.split_whitespace().next().unwrap_or_default();
    token.starts_with('/') && !input_classifier.is_slash_control_candidate(token)
}

fn candidate_line_status(bytes: &[u8]) -> CandidateLineStatus {
    if bytes.len() > 4096 {
        return CandidateLineStatus::Unsafe;
    }

    let Some(newline_idx) = bytes.iter().position(|byte| matches!(byte, b'\n' | b'\r')) else {
        if bytes
            .iter()
            .any(|byte| *byte == 0x1b || (*byte < 0x20 && !matches!(byte, b'\t')))
        {
            return CandidateLineStatus::Unsafe;
        }
        return CandidateLineStatus::Pending;
    };

    let line_len = newline_idx + 1;
    let line_bytes = &bytes[..line_len];
    if line_bytes
        .iter()
        .any(|byte| *byte == 0x1b || (*byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t')))
    {
        return CandidateLineStatus::Unsafe;
    }

    let Some(line) = std::str::from_utf8(line_bytes).ok() else {
        return CandidateLineStatus::Unsafe;
    };
    CandidateLineStatus::Complete {
        line: line.trim_end_matches(['\r', '\n']).to_string(),
        line_len,
    }
}

fn send_held_input_events(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    send_raw_input_events(bytes, input_events);
    if held_input_requests_cancel(bytes) {
        let _ = input_events.send(RawInputEvent::CtrlC);
    }
}

fn held_input_requests_cancel(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes)
        .lines()
        .any(|line| line.split_whitespace().next() == Some("/cancel"))
}

fn relay_delayed_input(
    bytes: &[u8],
    master: &mut File,
    input_classifier: &InputClassifier,
    input_events: &Sender<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
    line_buffer: &mut CandidateLineBuffer,
    native_line_state: &mut NativeLineState,
    exit_tracker: &mut ExplicitExitTracker,
) -> io::Result<()> {
    if bytes.contains(&CTRL_C) {
        let _ = input_events.send(RawInputEvent::CtrlC);
        line_buffer.clear();
        native_line_state.clear();
        return Ok(());
    }
    if relay_passthrough_input(
        bytes,
        master,
        input_classifier,
        input_events,
        input_mode,
        line_buffer,
        native_line_state,
        exit_tracker,
    )? {
        return Ok(());
    }
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
        let result = (|| {
            let mut card_state = CardInputState::default();
            let mut line_buffer = CandidateLineBuffer::default();
            let mut native_line_state = NativeLineState::default();
            let mut exit_tracker = ExplicitExitTracker::default();
            for action in actions {
                match action {
                    RawRelayAction::Write(bytes) => match current_raw_input_mode(&input_mode) {
                        RawInputMode::Capture(capture) => {
                            card_state.apply_capture(&capture);
                            let events = card_state.consume(&capture, &bytes);
                            if events.iter().any(releases_mode_capture) {
                                if let Ok(mut mode) = input_mode.lock() {
                                    *mode = RawInputMode::Passthrough;
                                }
                                card_state.reset();
                            }
                            for event in events {
                                let _ = input_events.send(event);
                            }
                        }
                        RawInputMode::Hold => {
                            card_state.reset();
                            send_held_input_events(&bytes, &input_events);
                        }
                        RawInputMode::Delay => {
                            card_state.reset();
                            relay_delayed_input(
                                &bytes,
                                &mut master,
                                &input_classifier,
                                &input_events,
                                &input_mode,
                                &mut line_buffer,
                                &mut native_line_state,
                                &mut exit_tracker,
                            )?;
                        }
                        RawInputMode::Passthrough => {
                            card_state.reset();
                            if relay_passthrough_input(
                                &bytes,
                                &mut master,
                                &input_classifier,
                                &input_events,
                                &input_mode,
                                &mut line_buffer,
                                &mut native_line_state,
                                &mut exit_tracker,
                            )? {
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
        })();
        result
    })
}

fn releases_mode_capture(event: &RawInputEvent) -> bool {
    matches!(
        event,
        RawInputEvent::ModeSet(_, _) | RawInputEvent::ModeCancel(_)
    )
}

#[derive(Debug, Default)]
struct ExplicitExitTracker {
    pending_line: Vec<u8>,
    saw_explicit_exit: bool,
}

impl ExplicitExitTracker {
    fn observe_shell_bytes(&mut self, bytes: &[u8]) {
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

    fn saw_explicit_exit(&self) -> bool {
        self.saw_explicit_exit
    }
}

fn is_explicit_exit_line(line: &[u8]) -> bool {
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim();
    trimmed == "exit" || trimmed.starts_with("exit ") || trimmed == "logout"
}

pub(crate) fn set_pty_winsize(fd: i32, winsize: Winsize) -> io::Result<()> {
    let result = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as libc::c_ulong, &winsize) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn signal_process_group(child_pid: u32, signal: i32) -> io::Result<()> {
    let result = unsafe { libc::kill(-(child_pid as i32), signal) };
    if result < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_inline_hint, native_candidate_should_return_to_shell,
        starts_native_intercept_candidate, CandidateLineBuffer, ExplicitExitTracker,
        NativeLineState,
    };
    use crate::input::InputClassifier;

    #[test]
    fn bare_slash_has_no_inline_hint() {
        assert_eq!(candidate_inline_hint("/"), None);
        assert_eq!(candidate_inline_hint("  /"), None);
        assert_eq!(
            candidate_inline_hint("/mo"),
            Some("/mode [recommend|agent]".to_string())
        );
    }

    #[test]
    fn native_slash_candidate_only_starts_at_line_start() {
        let mut state = NativeLineState::default();

        assert!(starts_native_intercept_candidate(b"/", &state));
        assert!(starts_native_intercept_candidate(b"?? hello", &state));

        state.observe_shell_bytes(b"vim .");
        assert!(!starts_native_intercept_candidate(b"/", &state));
        assert!(!starts_native_intercept_candidate(b"?? hello", &state));

        state.observe_shell_bytes(b"\n");
        assert!(starts_native_intercept_candidate(b"/mode", &state));
    }

    #[test]
    fn native_slash_candidate_returns_paths_and_tab_to_shell() {
        let classifier = InputClassifier::conservative();
        let mut line = CandidateLineBuffer::default();

        line.push(b"/m");
        assert!(!native_candidate_should_return_to_shell(&classifier, &line));

        line.push(b"ode agent");
        assert!(!native_candidate_should_return_to_shell(&classifier, &line));

        line.clear();
        line.push(b"/Users");
        assert!(native_candidate_should_return_to_shell(&classifier, &line));

        line.clear();
        line.push(b"/tmp/");
        assert!(native_candidate_should_return_to_shell(&classifier, &line));

        line.clear();
        line.push(b"/\t");
        assert!(native_candidate_should_return_to_shell(&classifier, &line));
    }

    #[test]
    fn explicit_exit_tracker_detects_split_exit_zero() {
        let mut tracker = ExplicitExitTracker::default();

        tracker.observe_shell_bytes(b"ex");
        assert!(!tracker.saw_explicit_exit());
        tracker.observe_shell_bytes(b"it 0\n");

        assert!(tracker.saw_explicit_exit());
    }

    #[test]
    fn explicit_exit_tracker_ignores_non_exit_lines() {
        let mut tracker = ExplicitExitTracker::default();

        tracker.observe_shell_bytes(b"echo exit\n");
        tracker.observe_shell_bytes(b"printf logout\n");

        assert!(!tracker.saw_explicit_exit());
    }
}
