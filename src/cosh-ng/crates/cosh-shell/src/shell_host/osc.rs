use std::collections::HashSet;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::model::ShellEnvironmentObserver;
use crate::types::{
    CommandOrigin, ShellEnvironmentSnapshot, ShellEvent, ShellEventKind, ShellHandoffRequest,
    SESSION_OUTPUT_REF_MAX_BYTES,
};

#[cfg(test)]
pub(super) use super::osc_output::{capped_output_ref_bytes, write_output_ref};
pub(super) use super::osc_output::{
    write_output_ref_with_session_cap, OutputRefCapture, OutputRefCaptureStatus,
};

const OSC_PREFIX: &[u8] = b"\x1b]1337;COSH;";
const BRACKETED_PASTE_ENABLE: &[u8] = b"\x1b[?2004h";
const BRACKETED_PASTE_DISABLE: &[u8] = b"\x1b[?2004l";
const STYLE_RESET: &[u8] = b"\x1b[0m";
const REVERSE_OFF: &[u8] = b"\x1b[27m";
const UNDERLINE_OFF: &[u8] = b"\x1b[24m";
const ERASE_TO_END_OF_SCREEN: &[u8] = b"\x1b[J";
const ERASE_TO_END_OF_LINE: &[u8] = b"\x1b[K";
const BEL: u8 = b'\x07';
const SHELL_PATH_MAX_BYTES: usize = 8 * 1024;

#[derive(Debug)]
struct CurrentCommand {
    id: String,
    command: String,
    cwd: String,
    origin: CommandOrigin,
    started_at_ms: u64,
    output_start: usize,
    shell_environment_generation: Option<u64>,
}

#[derive(Debug)]
pub(super) struct OscParser {
    pub(super) session_id: String,
    output_ref_dir: PathBuf,
    pub(super) events: Vec<ShellEvent>,
    pub(super) clean: Vec<u8>,
    pub(super) display: Vec<u8>,
    marker_token: String,
    pending: Vec<u8>,
    pending_clean_control: Vec<u8>,
    current: Option<CurrentCommand>,
    command_seq: usize,
    intervention_cuts: Vec<usize>,
    intervention_display_cuts: Vec<usize>,
    last_prompt_display_start: Option<usize>,
    pub(super) captured_output_ref_bytes: usize,
    pending_command_origin: Option<PendingCommandOrigin>,
    pending_handoff_echo: Option<PendingHandoffEcho>,
    pub(super) shell_environment_snapshot: Option<ShellEnvironmentSnapshot>,
    environment_observer: Option<ShellEnvironmentObserver>,
}

#[derive(Debug, Clone)]
struct PendingCommandOrigin {
    command: String,
    origin: CommandOrigin,
}

#[derive(Debug, Clone)]
struct PendingHandoffEcho {
    command: Vec<u8>,
    replacement: Vec<u8>,
    matched: usize,
    ansi_after_command: bool,
}

enum PendingHandoffEchoAction {
    Continue,
    PassThrough(u8),
    Complete(Vec<u8>),
    Mismatch(Vec<u8>),
}

impl OscParser {
    pub(super) fn new(session_id: String, output_ref_dir: PathBuf, marker_token: String) -> Self {
        Self {
            session_id,
            output_ref_dir,
            events: Vec::new(),
            clean: Vec::new(),
            display: Vec::new(),
            marker_token,
            pending: Vec::new(),
            pending_clean_control: Vec::new(),
            current: None,
            command_seq: 0,
            intervention_cuts: Vec::new(),
            intervention_display_cuts: Vec::new(),
            last_prompt_display_start: None,
            captured_output_ref_bytes: 0,
            pending_command_origin: None,
            pending_handoff_echo: None,
            shell_environment_snapshot: None,
            environment_observer: None,
        }
    }

    pub(super) fn with_environment_observer(mut self, observer: ShellEnvironmentObserver) -> Self {
        self.environment_observer = Some(observer);
        self
    }

    pub(super) fn register_pending_handoff_origin(&mut self, request: &ShellHandoffRequest) {
        self.pending_command_origin = Some(PendingCommandOrigin {
            command: request.command.clone(),
            origin: command_origin_from_handoff_request(request),
        });
    }

    pub(super) fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        self.pending.extend_from_slice(data);
        loop {
            let Some(start) = find_bytes(&self.pending, OSC_PREFIX) else {
                let keep = osc_prefix_suffix_len(&self.pending);
                let flush_len = self.pending.len().saturating_sub(keep);
                if flush_len > 0 {
                    let passthrough = self.pending[..flush_len].to_vec();
                    self.append_passthrough(&passthrough);
                    self.pending.drain(..flush_len);
                }
                return Ok(());
            };

            if start > 0 {
                let passthrough = self.pending[..start].to_vec();
                self.append_passthrough(&passthrough);
                self.pending.drain(..start);
            }

            let payload_start = OSC_PREFIX.len();
            let Some(end) = self.pending[payload_start..]
                .iter()
                .position(|byte| *byte == BEL)
                .map(|idx| idx + payload_start)
            else {
                return Ok(());
            };

            let payload = self.pending[payload_start..end].to_vec();
            self.pending.drain(..=end);
            match serde_json::from_slice::<Marker>(&payload) {
                Ok(marker) => self.handle_marker(marker)?,
                Err(err) => self.events.push(ShellEvent {
                    kind: ShellEventKind::ComponentFailed,
                    session_id: self.session_id.clone(),
                    command_id: None,
                    command: None,
                    cwd: None,
                    end_cwd: None,
                    exit_code: None,
                    started_at_ms: Some(now_ms()),
                    ended_at_ms: None,
                    duration_ms: None,
                    terminal_output_ref: None,
                    terminal_output_bytes: None,
                    input: None,
                    component: Some("osc_parser".to_string()),
                    message: Some(format!("marker parse failed: {err}")),
                    command_origin: None,
                    shell_environment_generation: None,
                }),
            }
        }
    }

    fn handle_marker(&mut self, marker: Marker) -> io::Result<()> {
        if marker.token.as_deref() != Some(self.marker_token.as_str()) {
            return Ok(());
        }

        if marker
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id != self.session_id)
        {
            return Ok(());
        }

        let environment_generation = self.observe_shell_environment(&marker);
        let session_id = marker
            .session_id
            .clone()
            .unwrap_or_else(|| self.session_id.clone());
        let timestamp = marker.timestamp_ms.unwrap_or_else(now_ms);

        match marker.event.as_str() {
            "intercept" => {
                let input = marker.command.unwrap_or_default();
                let reason = marker
                    .reason
                    .unwrap_or_else(|| "natural_language".to_string());
                self.intervention_cuts.push(self.clean.len());
                self.intervention_display_cuts.push(self.display.len());
                self.push_intercept_event(&session_id, input, marker.cwd, &reason);
                self.current = None;
            }
            "preexec" => {
                let command = marker.command.unwrap_or_default();
                self.command_seq += 1;
                let command_id = format!("cmd-{}", self.command_seq);
                let cwd = marker.cwd.unwrap_or_default();
                let origin = self.consume_pending_command_origin(&command);
                self.current = Some(CurrentCommand {
                    id: command_id.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                    origin,
                    started_at_ms: timestamp,
                    output_start: self.clean.len(),
                    shell_environment_generation: marker
                        .path_trusted
                        .unwrap_or(false)
                        .then_some(environment_generation)
                        .flatten(),
                });
                let mut event = ShellEvent::command_started_with_origin(
                    session_id, command_id, command, cwd, timestamp, origin,
                );
                event.shell_environment_generation = self
                    .current
                    .as_ref()
                    .and_then(|current| current.shell_environment_generation);
                self.events.push(event);
            }
            "precmd" => {
                let Some(current) = self.current.take() else {
                    self.intervention_cuts.push(self.clean.len());
                    self.intervention_display_cuts.push(self.display.len());
                    self.last_prompt_display_start = Some(self.display.len());
                    self.events.push(ShellEvent {
                        kind: ShellEventKind::ShellReady,
                        session_id,
                        command_id: None,
                        command: None,
                        cwd: marker.cwd,
                        end_cwd: None,
                        exit_code: None,
                        started_at_ms: Some(timestamp),
                        ended_at_ms: None,
                        duration_ms: None,
                        terminal_output_ref: None,
                        terminal_output_bytes: None,
                        input: None,
                        component: None,
                        message: None,
                        command_origin: None,
                        shell_environment_generation: None,
                    });
                    return Ok(());
                };

                let status = if is_shell_exit_command(&current.command) {
                    0
                } else {
                    marker.status.unwrap_or(0)
                };
                let output = self.clean[current.output_start..].to_vec();
                let output_ref = self.capture_command_output_ref(&current.id, &output)?;
                self.intervention_cuts.push(self.clean.len());
                self.intervention_display_cuts.push(self.display.len());
                self.last_prompt_display_start = Some(self.display.len());
                let kind = if status == 0 {
                    ShellEventKind::CommandCompleted
                } else {
                    ShellEventKind::CommandFailed
                };

                let mut event = command_finished_event(
                    kind,
                    session_id,
                    current.id,
                    status,
                    timestamp,
                    &output_ref,
                );
                event.command = Some(current.command);
                event.cwd = Some(current.cwd.clone());
                event.end_cwd = marker.cwd.or(Some(current.cwd));
                event.duration_ms = Some(timestamp.saturating_sub(current.started_at_ms));
                event.terminal_output_bytes = Some(output.len() as u64);
                event.command_origin = Some(current.origin);
                event.shell_environment_generation = current.shell_environment_generation;
                self.events.push(event);
            }
            _ => {}
        }

        Ok(())
    }

    fn observe_shell_environment(&mut self, marker: &Marker) -> Option<u64> {
        if !matches!(marker.event.as_str(), "precmd" | "preexec") {
            return None;
        }
        if marker.session_id.as_deref() != Some(self.session_id.as_str()) {
            return None;
        }
        let path = marker.path.as_deref()?;
        if path.len() > SHELL_PATH_MAX_BYTES {
            return None;
        }
        let normalized = normalize_shell_path(path);
        let marker_sequence = self
            .shell_environment_snapshot
            .as_ref()
            .map_or(Some(1), |snapshot| snapshot.marker_sequence.checked_add(1))?;
        let generation = self
            .shell_environment_snapshot
            .as_ref()
            .map_or(Some(1), |snapshot| {
                if snapshot.path == normalized {
                    Some(snapshot.generation)
                } else {
                    snapshot.generation.checked_add(1)
                }
            })?;
        let snapshot = ShellEnvironmentSnapshot {
            session_id: self.session_id.clone(),
            marker_sequence,
            generation,
            path: normalized,
        };
        self.shell_environment_snapshot = Some(snapshot.clone());
        if let Some(observer) = &self.environment_observer {
            observer.observe(snapshot);
        }
        Some(generation)
    }

    fn consume_pending_command_origin(&mut self, command: &str) -> CommandOrigin {
        let Some(pending) = self.pending_command_origin.take() else {
            return CommandOrigin::UserInteractive;
        };
        if pending.command == command {
            pending.origin
        } else {
            CommandOrigin::Unknown
        }
    }

    fn capture_command_output_ref(
        &mut self,
        command_id: &str,
        output: &[u8],
    ) -> io::Result<OutputRefCapture> {
        let capture = write_output_ref_with_session_cap(
            &self.output_ref_dir,
            command_id,
            output,
            self.captured_output_ref_bytes,
            SESSION_OUTPUT_REF_MAX_BYTES,
        )?;
        self.captured_output_ref_bytes = self
            .captured_output_ref_bytes
            .saturating_add(capture.captured_bytes);
        Ok(capture)
    }

    pub(super) fn flush_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        self.append_passthrough(&pending);
        self.flush_pending_clean_control();
    }

    fn append_passthrough(&mut self, data: &[u8]) {
        let data = self.filter_pending_handoff_echo(data);
        if data.is_empty() {
            return;
        }
        self.display.extend_from_slice(&data);
        self.append_clean(&data);
    }

    fn filter_pending_handoff_echo(&mut self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(data.len());
        for byte in data.iter().copied() {
            let Some(action) = self.pending_handoff_echo_action(byte) else {
                output.push(byte);
                continue;
            };
            match action {
                PendingHandoffEchoAction::Continue => {}
                PendingHandoffEchoAction::PassThrough(byte) => output.push(byte),
                PendingHandoffEchoAction::Complete(replacement) => {
                    output.extend_from_slice(&replacement);
                    self.pending_handoff_echo = None;
                }
                PendingHandoffEchoAction::Mismatch(bytes) => {
                    output.extend_from_slice(&bytes);
                    self.pending_handoff_echo = None;
                }
            }
        }
        output
    }

    fn pending_handoff_echo_action(&mut self, byte: u8) -> Option<PendingHandoffEchoAction> {
        let echo = self.pending_handoff_echo.as_mut()?;
        if echo.matched < echo.command.len() {
            if byte == echo.command[echo.matched] {
                echo.matched += 1;
                return Some(PendingHandoffEchoAction::Continue);
            }
            if echo.matched == 0 {
                return Some(PendingHandoffEchoAction::PassThrough(byte));
            }
            let mut bytes = echo.command[..echo.matched].to_vec();
            bytes.push(byte);
            return Some(PendingHandoffEchoAction::Mismatch(bytes));
        }

        if byte == b'\r' || byte == b'\n' {
            let mut replacement = echo.replacement.clone();
            replacement.push(byte);
            return Some(PendingHandoffEchoAction::Complete(replacement));
        }
        if byte == b'\x1b' {
            echo.ansi_after_command = true;
            return Some(PendingHandoffEchoAction::Continue);
        }
        if echo.ansi_after_command {
            if byte == b'[' || byte == b'?' || byte == b';' || byte.is_ascii_digit() {
                return Some(PendingHandoffEchoAction::Continue);
            }
            if (0x40..=0x7e).contains(&byte) {
                echo.ansi_after_command = false;
            }
            return Some(PendingHandoffEchoAction::Continue);
        }

        let mut bytes = echo.command.clone();
        bytes.push(byte);
        Some(PendingHandoffEchoAction::Mismatch(bytes))
    }

    fn append_clean(&mut self, data: &[u8]) {
        let mut bytes = Vec::new();
        if !self.pending_clean_control.is_empty() {
            bytes.append(&mut self.pending_clean_control);
        }
        bytes.extend_from_slice(data);

        let mut idx = 0;
        while idx < bytes.len() {
            let rest = &bytes[idx..];
            if let Some(control_len) = known_clean_control_len(rest) {
                idx += control_len;
                continue;
            }
            if is_known_clean_control_prefix(rest) {
                self.pending_clean_control.extend_from_slice(rest);
                return;
            }

            self.push_clean_byte(bytes[idx]);
            idx += 1;
        }
    }

    fn push_clean_byte(&mut self, byte: u8) {
        if byte == b'\x08' {
            pop_last_utf8_char(&mut self.clean);
            return;
        }
        self.clean.push(byte);
    }

    fn flush_pending_clean_control(&mut self) {
        let pending = std::mem::take(&mut self.pending_clean_control);
        for byte in pending {
            self.push_clean_byte(byte);
        }
    }

    pub(super) fn finish_current_on_exit(&mut self, status: i32) -> io::Result<()> {
        let Some(current) = self.current.take() else {
            return Ok(());
        };

        let ended_at = now_ms();
        let output = self.clean[current.output_start..].to_vec();
        let output_ref = self.capture_command_output_ref(&current.id, &output)?;
        let status = if is_shell_exit_command(&current.command) {
            0
        } else {
            status
        };
        let kind = if status == 0 {
            ShellEventKind::CommandCompleted
        } else {
            ShellEventKind::CommandFailed
        };
        let mut event = command_finished_event(
            kind,
            self.session_id.clone(),
            current.id,
            status,
            ended_at,
            &output_ref,
        );
        event.command = Some(current.command);
        event.cwd = Some(current.cwd.clone());
        event.end_cwd = Some(current.cwd);
        event.duration_ms = Some(ended_at.saturating_sub(current.started_at_ms));
        event.terminal_output_bytes = Some(output.len() as u64);
        event.command_origin = Some(current.origin);
        event.shell_environment_generation = current.shell_environment_generation;
        self.events.push(event);
        Ok(())
    }

    pub(super) fn prompt_count(&self, prompt: &[u8]) -> usize {
        if prompt.is_empty() {
            return 0;
        }
        self.clean
            .windows(prompt.len())
            .filter(|window| *window == prompt)
            .count()
    }

    pub(super) fn precmd_count(&self) -> usize {
        self.events
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    ShellEventKind::CommandCompleted
                        | ShellEventKind::CommandFailed
                        | ShellEventKind::ShellReady
                )
            })
            .count()
    }

    pub(super) fn drain_intervention_display_cuts(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.intervention_display_cuts)
    }

    pub(super) fn last_prompt_display(&self) -> &[u8] {
        let Some(start) = self.last_prompt_display_start else {
            return &[];
        };
        if start >= self.display.len() {
            return &[];
        }
        &self.display[start..]
    }

    pub(super) fn push_intercept_event(
        &mut self,
        session_id: &str,
        input: String,
        cwd: Option<String>,
        reason: &str,
    ) {
        self.events.push(ShellEvent {
            kind: ShellEventKind::UserInputIntercepted,
            session_id: session_id.to_string(),
            command_id: None,
            command: None,
            cwd,
            end_cwd: None,
            exit_code: None,
            started_at_ms: Some(now_ms()),
            ended_at_ms: None,
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: None,
            input: Some(input),
            component: Some(reason.to_string()),
            message: Some("input intercepted before reaching bash".to_string()),
            command_origin: None,
            shell_environment_generation: None,
        });
    }

    pub(super) fn push_control_event(&mut self, input: &str) {
        self.push_self_session_input_event(
            "control",
            "control input observed while relaying to bash",
            Some(input),
        );
    }

    pub(super) fn push_shell_input_activity_event(&mut self) {
        self.push_self_session_input_event(
            "shell_input",
            "user input observed while relaying to shell",
            None,
        );
    }

    pub(super) fn push_card_event(&mut self, action: &str, value: &str) {
        self.push_self_session_input_event("card", action, Some(value));
    }

    pub(super) fn push_prompt_ghost_event(&mut self, action: &str) {
        self.push_self_session_input_event("prompt_ghost", action, None);
    }

    fn push_self_session_input_event(
        &mut self,
        component: &str,
        message: &str,
        input: Option<&str>,
    ) {
        self.events.push(ShellEvent {
            kind: ShellEventKind::UserInputIntercepted,
            session_id: self.session_id.clone(),
            command_id: None,
            command: None,
            cwd: None,
            end_cwd: None,
            exit_code: None,
            started_at_ms: Some(now_ms()),
            ended_at_ms: None,
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: None,
            input: input.map(str::to_string),
            component: Some(component.to_string()),
            message: Some(message.to_string()),
            command_origin: None,
            shell_environment_generation: None,
        });
    }
}

fn is_shell_exit_command(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed == "exit" || trimmed.starts_with("exit ") || trimmed == "logout"
}

#[derive(Debug, Deserialize)]
struct Marker {
    event: String,
    token: Option<String>,
    session_id: Option<String>,
    timestamp_ms: Option<u64>,
    cwd: Option<String>,
    command: Option<String>,
    reason: Option<String>,
    status: Option<i32>,
    path: Option<String>,
    path_trusted: Option<bool>,
}

fn command_finished_event(
    kind: ShellEventKind,
    session_id: String,
    command_id: String,
    exit_code: i32,
    ended_at_ms: u64,
    output_ref: &OutputRefCapture,
) -> ShellEvent {
    match &output_ref.path {
        Some(path) => ShellEvent::command_finished(
            kind,
            session_id,
            command_id,
            exit_code,
            ended_at_ms,
            path.display().to_string(),
        ),
        None => ShellEvent {
            kind,
            session_id,
            command_id: Some(command_id),
            command: None,
            cwd: None,
            end_cwd: None,
            exit_code: Some(exit_code),
            started_at_ms: None,
            ended_at_ms: Some(ended_at_ms),
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: Some(0),
            input: None,
            component: Some("output_capture".to_string()),
            message: Some(match output_ref.status {
                OutputRefCaptureStatus::Captured => "output_capture_status: captured".to_string(),
                OutputRefCaptureStatus::SessionCapReached => {
                    "output_capture_status: unavailable; reason: session_output_cap_reached"
                        .to_string()
                }
            }),
            command_origin: None,
            shell_environment_generation: None,
        },
    }
}

fn normalize_shell_path(path: &str) -> String {
    let mut seen = HashSet::new();
    path.split(':')
        .filter_map(normalize_absolute_path)
        .filter(|entry| seen.insert(entry.clone()))
        .collect::<Vec<_>>()
        .join(":")
}

fn normalize_absolute_path(value: &str) -> Option<String> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => normalized.push(".."),
            Component::Normal(part) => normalized.push(part),
            Component::Prefix(_) => return None,
        }
    }
    Some(normalized.to_string_lossy().into_owned())
}

fn command_origin_from_handoff_request(request: &ShellHandoffRequest) -> CommandOrigin {
    match request.source.as_str() {
        "send_to_shell" => CommandOrigin::UserSendToShell,
        "user_analysis_action" => CommandOrigin::UserAnalysisAction,
        "approved_provider_shell_tool" => CommandOrigin::ProviderTool,
        "approved_fallback" => CommandOrigin::AgentHandoff,
        "validation" => CommandOrigin::ShellInternal,
        _ => CommandOrigin::Unknown,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn osc_prefix_suffix_len(pending: &[u8]) -> usize {
    let max_keep = pending.len().min(OSC_PREFIX.len().saturating_sub(1));
    for size in (1..=max_keep).rev() {
        if OSC_PREFIX.starts_with(&pending[pending.len() - size..]) {
            return size;
        }
    }
    0
}

fn known_clean_control_len(bytes: &[u8]) -> Option<usize> {
    [
        BRACKETED_PASTE_ENABLE,
        BRACKETED_PASTE_DISABLE,
        STYLE_RESET,
        REVERSE_OFF,
        UNDERLINE_OFF,
        ERASE_TO_END_OF_SCREEN,
        ERASE_TO_END_OF_LINE,
    ]
    .into_iter()
    .find(|control| bytes.starts_with(control))
    .map(|control| control.len())
}

fn is_known_clean_control_prefix(bytes: &[u8]) -> bool {
    [
        BRACKETED_PASTE_ENABLE,
        BRACKETED_PASTE_DISABLE,
        STYLE_RESET,
        REVERSE_OFF,
        UNDERLINE_OFF,
        ERASE_TO_END_OF_SCREEN,
        ERASE_TO_END_OF_LINE,
    ]
    .into_iter()
    .any(|control| control.starts_with(bytes))
}

fn pop_last_utf8_char(bytes: &mut Vec<u8>) {
    while let Some(byte) = bytes.pop() {
        if byte & 0b1100_0000 != 0b1000_0000 {
            break;
        }
    }
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
