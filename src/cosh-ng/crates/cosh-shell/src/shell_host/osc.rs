use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::types::{
    ShellEvent, ShellEventKind, COMMAND_OUTPUT_REF_MAX_BYTES, SESSION_OUTPUT_REF_MAX_BYTES,
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

#[derive(Debug)]
struct CurrentCommand {
    id: String,
    command: String,
    cwd: String,
    started_at_ms: u64,
    output_start: usize,
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
    captured_output_ref_bytes: usize,
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
        }
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
                }),
            }
        }
    }

    fn handle_marker(&mut self, marker: Marker) -> io::Result<()> {
        if marker.token.as_deref() != Some(self.marker_token.as_str()) {
            return Ok(());
        }

        let session_id = marker.session_id.unwrap_or_else(|| self.session_id.clone());
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
                if is_internal_restore_command(&command) {
                    self.current = None;
                    return Ok(());
                }

                self.command_seq += 1;
                let command_id = format!("cmd-{}", self.command_seq);
                let cwd = marker.cwd.unwrap_or_default();
                self.current = Some(CurrentCommand {
                    id: command_id.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                    started_at_ms: timestamp,
                    output_start: self.clean.len(),
                });
                self.events.push(ShellEvent::command_started(
                    session_id, command_id, command, cwd, timestamp,
                ));
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
                self.events.push(event);
            }
            _ => {}
        }

        Ok(())
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
        self.display.extend_from_slice(data);
        self.append_clean(data);
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
        });
    }

    pub(super) fn push_control_event(&mut self, input: &str) {
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
            input: Some(input.to_string()),
            component: Some("control".to_string()),
            message: Some("control input observed while relaying to bash".to_string()),
        });
    }

    pub(super) fn push_card_event(&mut self, action: &str, value: &str) {
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
            input: Some(value.to_string()),
            component: Some("card".to_string()),
            message: Some(action.to_string()),
        });
    }
}

fn is_shell_exit_command(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed == "exit" || trimmed.starts_with("exit ") || trimmed == "logout"
}

fn is_internal_restore_command(command: &str) -> bool {
    command
        .trim_start()
        .starts_with("COSH_INTERNAL_RESTORE=1 stty echo icanon isig iexten opost 2>/dev/null")
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
}

#[cfg(test)]
fn write_output_ref(dir: &Path, command_id: &str, output: &[u8]) -> io::Result<PathBuf> {
    Ok(
        write_output_ref_with_session_cap(dir, command_id, output, 0, usize::MAX)?
            .path
            .expect("unbounded session cap should capture output ref"),
    )
}

#[derive(Debug)]
struct OutputRefCapture {
    path: Option<PathBuf>,
    captured_bytes: usize,
    status: OutputRefCaptureStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputRefCaptureStatus {
    Captured,
    SessionCapReached,
}

fn write_output_ref_with_session_cap(
    dir: &Path,
    command_id: &str,
    output: &[u8],
    session_captured_bytes: usize,
    session_cap_bytes: usize,
) -> io::Result<OutputRefCapture> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let captured = capped_output_ref_bytes(output, COMMAND_OUTPUT_REF_MAX_BYTES);
    if session_captured_bytes.saturating_add(captured.len()) > session_cap_bytes {
        return Ok(OutputRefCapture {
            path: None,
            captured_bytes: 0,
            status: OutputRefCaptureStatus::SessionCapReached,
        });
    }

    let path = dir.join(format!("{command_id}.txt"));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)?;
    file.write_all(&captured)?;
    file.sync_all()?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(OutputRefCapture {
        path: Some(path),
        captured_bytes: captured.len(),
        status: OutputRefCaptureStatus::Captured,
    })
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
        },
    }
}

fn capped_output_ref_bytes(output: &[u8], max_bytes: usize) -> Vec<u8> {
    if output.len() <= max_bytes {
        return output.to_vec();
    }

    let marker = format!(
        "\n[captured output truncated: original_bytes={}, max_capture_bytes={}]\n",
        output.len(),
        max_bytes
    )
    .into_bytes();
    if max_bytes <= marker.len() {
        return marker[..max_bytes].to_vec();
    }

    let available = max_bytes - marker.len();
    let head_len = utf8_floor_boundary(output, available / 2);
    let tail_len = available.saturating_sub(head_len);
    let tail_start = utf8_ceil_boundary(output, output.len().saturating_sub(tail_len));

    let mut captured = Vec::with_capacity(max_bytes);
    captured.extend_from_slice(&output[..head_len]);
    captured.extend_from_slice(&marker);
    captured.extend_from_slice(&output[tail_start..]);
    captured
}

fn utf8_floor_boundary(bytes: &[u8], mut index: usize) -> usize {
    index = index.min(bytes.len());
    while index > 0 && index < bytes.len() && is_utf8_continuation(bytes[index]) {
        index -= 1;
    }
    index
}

fn utf8_ceil_boundary(bytes: &[u8], mut index: usize) -> usize {
    index = index.min(bytes.len());
    while index < bytes.len() && is_utf8_continuation(bytes[index]) {
        index += 1;
    }
    index
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const TEST_MARKER_TOKEN: &str = "test-marker-token";

    #[test]
    fn parser_clean_strips_zsh_bracketed_paste_and_applies_backspace() {
        let mut parser = parser_for_test("clean-zsh-control");
        let input =
            b"\x1b[0m\x1b[27m\x1b[24m\x1b[Jcosh-osc$ \x1b[K\x1b[?2004he\x08echo ok\x1b[?2004l\r\n";

        parser.feed(input).expect("feed");

        assert_eq!(
            String::from_utf8_lossy(&parser.clean),
            "cosh-osc$ echo ok\r\n"
        );
        assert_eq!(parser.display, input);
    }

    #[test]
    fn parser_clean_handles_split_zsh_bracketed_paste_control() {
        let mut parser = parser_for_test("clean-zsh-split-control");

        parser.feed(b"\x1b[?20").expect("feed partial");
        assert!(parser.clean.is_empty());
        parser.feed(b"04hcmd\x1b[?2004l").expect("feed remainder");

        assert_eq!(String::from_utf8_lossy(&parser.clean), "cmd");
    }

    #[test]
    fn precmd_count_tracks_shell_ready_and_command_events() {
        let mut parser = parser_for_test("precmd-count");
        assert_eq!(parser.precmd_count(), 0);

        // ShellReady via precmd without a current command
        let mut precmd_no_cmd: Vec<u8> = Vec::new();
        precmd_no_cmd.extend_from_slice(b"\x1b]1337;COSH;");
        precmd_no_cmd
            .extend_from_slice(br#"{"event":"precmd","token":"test-marker-token","cwd":"/tmp"}"#);
        precmd_no_cmd.push(BEL);
        parser.feed(&precmd_no_cmd).expect("feed precmd");
        assert_eq!(parser.precmd_count(), 1);

        // CommandCompleted via preexec + precmd with status 0
        let mut preexec: Vec<u8> = Vec::new();
        preexec.extend_from_slice(b"\x1b]1337;COSH;");
        preexec.extend_from_slice(
            br#"{"event":"preexec","token":"test-marker-token","command":"echo hi","cwd":"/tmp"}"#,
        );
        preexec.push(BEL);
        parser.feed(&preexec).expect("feed preexec");
        assert_eq!(parser.precmd_count(), 1);

        let mut precmd_ok: Vec<u8> = Vec::new();
        precmd_ok.extend_from_slice(b"\x1b]1337;COSH;");
        precmd_ok.extend_from_slice(
            br#"{"event":"precmd","token":"test-marker-token","status":0,"cwd":"/tmp"}"#,
        );
        precmd_ok.push(BEL);
        parser.feed(&precmd_ok).expect("feed precmd ok");
        assert_eq!(parser.precmd_count(), 2);

        // CommandFailed via preexec + precmd with status 1
        let mut preexec2: Vec<u8> = Vec::new();
        preexec2.extend_from_slice(b"\x1b]1337;COSH;");
        preexec2.extend_from_slice(
            br#"{"event":"preexec","token":"test-marker-token","command":"false","cwd":"/tmp"}"#,
        );
        preexec2.push(BEL);
        parser.feed(&preexec2).expect("feed preexec2");

        let mut precmd_fail: Vec<u8> = Vec::new();
        precmd_fail.extend_from_slice(b"\x1b]1337;COSH;");
        precmd_fail.extend_from_slice(
            br#"{"event":"precmd","token":"test-marker-token","status":1,"cwd":"/tmp"}"#,
        );
        precmd_fail.push(BEL);
        parser.feed(&precmd_fail).expect("feed precmd fail");
        assert_eq!(parser.precmd_count(), 3);
    }

    #[test]
    fn output_ref_file_uses_private_permissions() {
        let dir =
            std::env::temp_dir().join(format!("cosh-shell-osc-output-ref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let path = write_output_ref(&dir, "cmd-1", b"secret-ish\n").expect("write output ref");

        assert_eq!(
            std::fs::metadata(&dir)
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_ref_file_is_capped_but_preserves_head_and_tail() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-osc-output-ref-cap-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut output = Vec::new();
        output.extend_from_slice(b"head-line\n");
        output.extend(std::iter::repeat(b'x').take(COMMAND_OUTPUT_REF_MAX_BYTES));
        output.extend_from_slice(b"\ntail-line\n");

        let path = write_output_ref(&dir, "cmd-1", &output).expect("write output ref");
        let captured = std::fs::read(&path).expect("read output ref");
        let captured_text = String::from_utf8(captured.clone()).expect("utf8 capped output");

        assert!(captured.len() <= COMMAND_OUTPUT_REF_MAX_BYTES);
        assert!(captured_text.starts_with("head-line"), "{captured_text}");
        assert!(
            captured_text.contains("[captured output truncated:"),
            "{captured_text}"
        );
        assert!(captured_text.ends_with("tail-line\n"), "{captured_text}");
        assert!(
            captured_text.contains(&format!("original_bytes={}", output.len())),
            "{captured_text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capped_output_ref_respects_utf8_boundaries() {
        let input = "头".repeat(COMMAND_OUTPUT_REF_MAX_BYTES / 3 + 10);

        let captured = capped_output_ref_bytes(input.as_bytes(), 4096);

        let captured_text = String::from_utf8(captured).expect("valid utf8");
        assert!(captured_text.contains("[captured output truncated:"));
        assert!(captured_text.starts_with('头'));
        assert!(captured_text.ends_with('头'));
    }

    #[test]
    fn output_ref_session_cap_marks_later_output_unavailable() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-osc-output-ref-session-cap-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let first =
            write_output_ref_with_session_cap(&dir, "cmd-1", b"12345", 0, 8).expect("first ref");
        let second =
            write_output_ref_with_session_cap(&dir, "cmd-2", b"6789", first.captured_bytes, 8)
                .expect("second ref");

        assert_eq!(first.status, OutputRefCaptureStatus::Captured);
        assert!(first.path.as_ref().is_some_and(|path| path.exists()));
        assert_eq!(second.status, OutputRefCaptureStatus::SessionCapReached);
        assert!(second.path.is_none());
        assert!(!dir.join("cmd-2.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parser_session_cap_preserves_command_facts_without_output_ref() {
        let mut parser = parser_for_test("session-cap-events");
        parser.captured_output_ref_bytes = SESSION_OUTPUT_REF_MAX_BYTES;

        let mut preexec: Vec<u8> = Vec::new();
        preexec.extend_from_slice(b"\x1b]1337;COSH;");
        preexec.extend_from_slice(
            br#"{"event":"preexec","token":"test-marker-token","command":"printf capped","cwd":"/tmp","timestamp_ms":10}"#,
        );
        preexec.push(BEL);
        parser.feed(&preexec).expect("feed preexec");
        parser.feed(b"captured body\n").expect("feed output");

        let mut precmd: Vec<u8> = Vec::new();
        precmd.extend_from_slice(b"\x1b]1337;COSH;");
        precmd.extend_from_slice(
            br#"{"event":"precmd","token":"test-marker-token","status":0,"cwd":"/tmp","timestamp_ms":20}"#,
        );
        precmd.push(BEL);
        parser.feed(&precmd).expect("feed precmd");

        let event = parser
            .events
            .iter()
            .find(|event| {
                matches!(
                    event.kind,
                    ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed
                ) && event.command_id.as_deref() == Some("cmd-1")
            })
            .expect("finished command event");
        assert_eq!(event.command.as_deref(), Some("printf capped"));
        assert_eq!(event.terminal_output_ref, None);
        assert_eq!(
            event.terminal_output_bytes,
            Some("captured body\n".len() as u64)
        );
        assert_eq!(event.component.as_deref(), Some("output_capture"));
        assert_eq!(
            event.message.as_deref(),
            Some("output_capture_status: unavailable; reason: session_output_cap_reached")
        );
    }

    fn parser_for_test(name: &str) -> OscParser {
        let dir =
            std::env::temp_dir().join(format!("cosh-shell-osc-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("output ref dir");
        OscParser::new(name.to_string(), dir, TEST_MARKER_TOKEN.to_string())
    }
}
