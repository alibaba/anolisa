use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::types::{ShellEvent, ShellEventKind};

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
    pending: Vec<u8>,
    pending_clean_control: Vec<u8>,
    current: Option<CurrentCommand>,
    command_seq: usize,
    intervention_cuts: Vec<usize>,
    intervention_display_cuts: Vec<usize>,
}

impl OscParser {
    pub(super) fn new(session_id: String, output_ref_dir: PathBuf) -> Self {
        Self {
            session_id,
            output_ref_dir,
            events: Vec::new(),
            clean: Vec::new(),
            display: Vec::new(),
            pending: Vec::new(),
            pending_clean_control: Vec::new(),
            current: None,
            command_seq: 0,
            intervention_cuts: Vec::new(),
            intervention_display_cuts: Vec::new(),
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
                self.command_seq += 1;
                let command_id = format!("cmd-{}", self.command_seq);
                let command = marker.command.unwrap_or_default();
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

                let status = marker.status.unwrap_or(0);
                let output = self.clean[current.output_start..].to_vec();
                let output_ref = write_output_ref(&self.output_ref_dir, &current.id, &output)?;
                self.intervention_cuts.push(self.clean.len());
                self.intervention_display_cuts.push(self.display.len());
                let kind = if status == 0 {
                    ShellEventKind::CommandCompleted
                } else {
                    ShellEventKind::CommandFailed
                };

                let mut event = ShellEvent::command_finished(
                    kind,
                    session_id,
                    current.id,
                    status,
                    timestamp,
                    output_ref.display().to_string(),
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
        let output_ref = write_output_ref(&self.output_ref_dir, &current.id, &output)?;
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
        let mut event = ShellEvent::command_finished(
            kind,
            self.session_id.clone(),
            current.id,
            status,
            ended_at,
            output_ref.display().to_string(),
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
        self.events.iter().filter(|e| {
            matches!(e.kind, ShellEventKind::CommandCompleted
                | ShellEventKind::CommandFailed
                | ShellEventKind::ShellReady)
        }).count()
    }

    pub(super) fn drain_intervention_display_cuts(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.intervention_display_cuts)
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

#[derive(Debug, Deserialize)]
struct Marker {
    event: String,
    session_id: Option<String>,
    timestamp_ms: Option<u64>,
    cwd: Option<String>,
    command: Option<String>,
    reason: Option<String>,
    status: Option<i32>,
}

fn write_output_ref(dir: &PathBuf, command_id: &str, output: &[u8]) -> io::Result<PathBuf> {
    let path = dir.join(format!("{command_id}.txt"));
    fs::write(&path, output)?;
    Ok(path)
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
        precmd_no_cmd.extend_from_slice(br#"{"event":"precmd","cwd":"/tmp"}"#);
        precmd_no_cmd.push(BEL);
        parser.feed(&precmd_no_cmd).expect("feed precmd");
        assert_eq!(parser.precmd_count(), 1);

        // CommandCompleted via preexec + precmd with status 0
        let mut preexec: Vec<u8> = Vec::new();
        preexec.extend_from_slice(b"\x1b]1337;COSH;");
        preexec.extend_from_slice(br#"{"event":"preexec","command":"echo hi","cwd":"/tmp"}"#);
        preexec.push(BEL);
        parser.feed(&preexec).expect("feed preexec");
        assert_eq!(parser.precmd_count(), 1);

        let mut precmd_ok: Vec<u8> = Vec::new();
        precmd_ok.extend_from_slice(b"\x1b]1337;COSH;");
        precmd_ok.extend_from_slice(br#"{"event":"precmd","status":0,"cwd":"/tmp"}"#);
        precmd_ok.push(BEL);
        parser.feed(&precmd_ok).expect("feed precmd ok");
        assert_eq!(parser.precmd_count(), 2);

        // CommandFailed via preexec + precmd with status 1
        let mut preexec2: Vec<u8> = Vec::new();
        preexec2.extend_from_slice(b"\x1b]1337;COSH;");
        preexec2.extend_from_slice(br#"{"event":"preexec","command":"false","cwd":"/tmp"}"#);
        preexec2.push(BEL);
        parser.feed(&preexec2).expect("feed preexec2");

        let mut precmd_fail: Vec<u8> = Vec::new();
        precmd_fail.extend_from_slice(b"\x1b]1337;COSH;");
        precmd_fail.extend_from_slice(br#"{"event":"precmd","status":1,"cwd":"/tmp"}"#);
        precmd_fail.push(BEL);
        parser.feed(&precmd_fail).expect("feed precmd fail");
        assert_eq!(parser.precmd_count(), 3);
    }

    fn parser_for_test(name: &str) -> OscParser {
        let dir =
            std::env::temp_dir().join(format!("cosh-shell-osc-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("output ref dir");
        OscParser::new(name.to_string(), dir)
    }
}
