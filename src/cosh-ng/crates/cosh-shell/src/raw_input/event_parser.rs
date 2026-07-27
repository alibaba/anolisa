use crate::input::InputClassifier;

use super::{CTRL_C, CTRL_U};

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

/// Soft newline marker inserted when user presses Alt+Enter or Shift+Enter.
/// Using VT (vertical tab, 0x0b) which is a control char that won't appear
/// in normal text input and won't trigger candidate_line_status Complete.
const SOFT_NEWLINE: u8 = 0x0b;

#[derive(Debug, Default)]
pub(super) struct CandidateLineBuffer {
    pub(super) bytes: Vec<u8>,
    pub(super) relayed_len: usize,
    pub(super) force_agent_intercept: bool,
    pub(super) forced_agent_suggestion_id: Option<String>,
}

impl CandidateLineBuffer {
    pub(super) fn is_active(&self) -> bool {
        !self.bytes.is_empty()
    }

    pub(super) fn push(&mut self, bytes: &[u8]) {
        let mut idx = 0;
        while idx < bytes.len() {
            if bytes[idx..].starts_with(BRACKETED_PASTE_START) {
                idx += BRACKETED_PASTE_START.len();
                continue;
            }
            if bytes[idx..].starts_with(BRACKETED_PASTE_END) {
                idx += BRACKETED_PASTE_END.len();
                continue;
            }
            match bytes[idx] {
                CTRL_U => {
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
                // Alt+Enter: ESC + CR or ESC + LF → soft newline
                0x1b if bytes.get(idx + 1) == Some(&b'\r')
                    || bytes.get(idx + 1) == Some(&b'\n') =>
                {
                    self.bytes.push(SOFT_NEWLINE);
                    idx += 2;
                }
                // Shift+Enter (kitty keyboard protocol): ESC [ 1 3 ; 2 u
                // or ESC [ 2 7 ; 2 u
                0x1b if is_shift_enter_csi(&bytes[idx..]) => {
                    self.bytes.push(SOFT_NEWLINE);
                    idx += shift_enter_csi_len(&bytes[idx..]);
                }
                byte => {
                    self.bytes.push(byte);
                    idx += 1;
                }
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.bytes.clear();
        self.relayed_len = 0;
        self.force_agent_intercept = false;
        self.forced_agent_suggestion_id = None;
    }

    pub(super) fn take(&mut self) -> Vec<u8> {
        self.relayed_len = 0;
        self.force_agent_intercept = false;
        self.forced_agent_suggestion_id = None;
        let mut bytes = std::mem::take(&mut self.bytes);
        // Convert soft newlines back to real newlines for downstream processing
        for byte in &mut bytes {
            if *byte == SOFT_NEWLINE {
                *byte = b'\n';
            }
        }
        bytes
    }

    pub(super) fn visible_line_bytes(&self) -> &[u8] {
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
pub(super) enum CandidateLineStatus {
    Pending,
    Complete { line: String, line_len: usize },
    Unsafe,
}

#[derive(Debug, Default)]
pub(super) struct NativeLineState {
    visible: Vec<u8>,
}

impl NativeLineState {
    fn is_at_line_start(&self) -> bool {
        self.visible.is_empty()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    pub(super) fn observe_shell_bytes(&mut self, bytes: &[u8]) {
        let mut idx = 0;
        while idx < bytes.len() {
            if bytes[idx..].starts_with(BRACKETED_PASTE_START) {
                idx += BRACKETED_PASTE_START.len();
                continue;
            }
            if bytes[idx..].starts_with(BRACKETED_PASTE_END) {
                idx += BRACKETED_PASTE_END.len();
                continue;
            }
            match bytes[idx] {
                CTRL_C | CTRL_U | b'\n' | b'\r' => {
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

    pub(super) fn clear(&mut self) {
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

pub(super) fn candidate_inline_hint(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('/') || trimmed[1..].contains('/') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let token = parts.next().unwrap_or_default();
    match token {
        "/" => None,
        "/mode" if parts.next().is_none() => {
            Some("approval [recommend|auto|trust] | analysis [smart|auto|manual]".to_string())
        }
        "/details" if parts.next().is_none() => Some("<id>".to_string()),
        "/aut" => Some("/auth".to_string()),
        _ => crate::slash::registry::visible_slash_commands()
            .find(|spec| spec.name.starts_with(token) && spec.name != token)
            .map(|spec| spec.usage.to_string()),
    }
}

pub(crate) fn redact_extension_setting_value(input: &[u8]) -> Vec<u8> {
    let mut tokens = Vec::with_capacity(6);
    let mut index = 0;
    while index < input.len() && tokens.len() < 6 {
        while index < input.len() && input[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == input.len() {
            break;
        }
        let start = index;
        while index < input.len() && !input[index].is_ascii_whitespace() {
            index += 1;
        }
        tokens.push((start, index));
    }

    if tokens.len() < 6
        || &input[tokens[0].0..tokens[0].1] != b"/extensions"
        || &input[tokens[1].0..tokens[1].1] != b"settings"
        || &input[tokens[2].0..tokens[2].1] != b"set"
    {
        return input.to_vec();
    }

    let value_start = tokens[5].0;
    let mut redacted = input.to_vec();
    for byte in &mut redacted[value_start..] {
        if !byte.is_ascii_whitespace() {
            *byte = b'*';
        }
    }
    redacted
}

pub(super) fn starts_intercept_candidate(bytes: &[u8]) -> bool {
    let first = first_visible_input_byte(bytes);
    matches!(first, Some(b'/' | b'?')) || first.is_some_and(|byte| byte >= 0x80)
}

pub(super) fn starts_native_intercept_candidate(
    bytes: &[u8],
    native_line_state: &NativeLineState,
) -> bool {
    native_line_state.is_at_line_start()
        && (first_visible_input_byte(bytes) == Some(b'/')
            || first_visible_input_bytes(bytes).starts_with(b"??"))
}

fn first_visible_input_byte(bytes: &[u8]) -> Option<u8> {
    first_visible_input_bytes(bytes).first().copied()
}

fn first_visible_input_bytes(mut bytes: &[u8]) -> &[u8] {
    loop {
        if bytes.starts_with(BRACKETED_PASTE_START) {
            bytes = &bytes[BRACKETED_PASTE_START.len()..];
            continue;
        }
        if bytes.starts_with(BRACKETED_PASTE_END) {
            bytes = &bytes[BRACKETED_PASTE_END.len()..];
            continue;
        }
        return bytes;
    }
}

pub(super) fn native_candidate_should_return_to_shell(
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

/// Check if bytes start with a Shift+Enter CSI sequence.
/// Recognizes: \x1b[13;2u (Shift+Enter) and \x1b[27;2u (Shift+Enter alt code).
fn is_shift_enter_csi(bytes: &[u8]) -> bool {
    matches!(
        bytes,
        [0x1b, b'[', b'1', b'3', b';', b'2', b'u', ..]
            | [0x1b, b'[', b'2', b'7', b';', b'2', b'u', ..]
    )
}

/// Return the length of a Shift+Enter CSI sequence at the start of bytes.
fn shift_enter_csi_len(bytes: &[u8]) -> usize {
    if bytes.starts_with(b"\x1b[13;2u") || bytes.starts_with(b"\x1b[27;2u") {
        7
    } else {
        1
    }
}

pub(super) fn candidate_line_status(bytes: &[u8]) -> CandidateLineStatus {
    if bytes.len() > 4096 {
        return CandidateLineStatus::Unsafe;
    }

    let Some(newline_idx) = bytes.iter().position(|byte| matches!(byte, b'\n' | b'\r')) else {
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == 0x1b {
                return if incomplete_escape_suffix(&bytes[index..]) {
                    CandidateLineStatus::Pending
                } else {
                    CandidateLineStatus::Unsafe
                };
            }
            if *byte < 0x20 && !matches!(*byte, b'\t' | SOFT_NEWLINE) {
                return CandidateLineStatus::Unsafe;
            }
        }
        return CandidateLineStatus::Pending;
    };

    let line_len = newline_idx + 1;
    let line_bytes = &bytes[..line_len];
    if line_bytes.iter().any(|byte| {
        *byte == 0x1b || (*byte < 0x20 && !matches!(*byte, b'\n' | b'\r' | b'\t' | SOFT_NEWLINE))
    }) {
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

fn incomplete_escape_suffix(bytes: &[u8]) -> bool {
    match bytes {
        [0x1b] => true,
        [0x1b, b'[', parameters @ ..] => parameters.iter().all(|byte| matches!(byte, 0x20..=0x3f)),
        [0x1b, b'O'] => true,
        _ => false,
    }
}
