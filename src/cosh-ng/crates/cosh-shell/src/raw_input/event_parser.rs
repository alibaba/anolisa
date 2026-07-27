use crate::input::InputClassifier;

use super::{CTRL_C, CTRL_U};

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

/// Escape sequences that terminals emit for Alt+Enter / Shift+Enter soft-newline
/// shortcuts.  The relay converts them to a literal `\n` in the candidate
/// buffer so the user can compose multi-line prompts.
const SOFT_NEWLINE_SEQUENCES: &[&[u8]] = &[
    b"\x1b\r",     // Alt+Enter (xterm / most terminals)
    b"\x1b\n",     // Alt+Enter variant
    b"\x1b[13;2u", // Shift+Enter  (xterm modifyOtherKeys / kitty)
    b"\x1b[13;3u", // Alt+Enter    (xterm modifyOtherKeys / kitty)
    b"\x1b[13;4u", // Shift+Alt+Enter
    b"\x1b[13;5u", // Ctrl+Enter
    b"\x1b[13;6u", // Shift+Ctrl+Enter
    b"\x1bO\r",    // SS3 prefix variant
    b"\x1bO\n",    // SS3 prefix variant
];

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

        // Handle split soft-newline: ESC already sits at the tail of the
        // buffer and the CR/LF (or the rest of a modifyOtherKeys sequence)
        // arrives in the next read.
        if !bytes.is_empty()
            && self.bytes.last() == Some(&0x1b)
            && consume_soft_newline_split(&bytes[..])
        {
            self.bytes.pop();
            self.bytes.push(b'\n');
            idx += skip_soft_newline_tail(&bytes[..]);
        }

        while idx < bytes.len() {
            if bytes[idx..].starts_with(BRACKETED_PASTE_START) {
                idx += BRACKETED_PASTE_START.len();
                continue;
            }
            if bytes[idx..].starts_with(BRACKETED_PASTE_END) {
                idx += BRACKETED_PASTE_END.len();
                continue;
            }

            // Soft newline: Alt+Enter / Shift+Enter escape sequences.
            if let Some(seq_len) = try_consume_soft_newline(&bytes[idx..]) {
                self.bytes.push(b'\n');
                idx += seq_len;
                continue;
            }

            // Soft newline (direction B): backslash + Enter (\r).
            if bytes[idx] == b'\r' && self.bytes.last() == Some(&b'\\') {
                self.bytes.pop();
                self.bytes.push(b'\n');
                idx += 1;
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
        std::mem::take(&mut self.bytes)
    }

    pub(super) fn visible_line_bytes(&self) -> &[u8] {
        // \n is a soft-newline inserted by Alt+Enter / Shift+Enter; only a
        // bare \r (Enter / accept-line) terminates the visible region.
        let end = self
            .bytes
            .iter()
            .position(|byte| matches!(byte, b'\r'))
            .unwrap_or(self.bytes.len());
        &self.bytes[..end]
    }

    fn pop_visible_char(&mut self) {
        let Some(end) = self
            .bytes
            .iter()
            .position(|byte| matches!(byte, b'\r'))
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

pub(super) fn candidate_line_status(bytes: &[u8]) -> CandidateLineStatus {
    if bytes.len() > 4096 {
        return CandidateLineStatus::Unsafe;
    }

    // Only a bare \r (Enter / accept-line) triggers submission.
    // \n bytes are soft-newlines inserted by Alt+Enter / Shift+Enter and
    // must remain part of the composed multi-line prompt.
    let Some(newline_idx) = bytes.iter().position(|byte| matches!(byte, b'\r')) else {
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == 0x1b {
                return if incomplete_escape_suffix(&bytes[index..]) {
                    CandidateLineStatus::Pending
                } else {
                    CandidateLineStatus::Unsafe
                };
            }
            // \n is a soft-newline (visible, not unsafe); \t is tab.
            if *byte < 0x20 && !matches!(byte, b'\t' | b'\n') {
                return CandidateLineStatus::Unsafe;
            }
        }
        return CandidateLineStatus::Pending;
    };

    let line_len = newline_idx + 1;
    let line_bytes = &bytes[..line_len];
    if line_bytes
        .iter()
        .any(|byte| *byte == 0x1b || (*byte < 0x20 && !matches!(byte, b'\r' | b'\n' | b'\t')))
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

/// Returns the length of a soft-newline escape sequence starting at `bytes`,
/// or `None` if the prefix does not match any known soft-newline sequence.
fn try_consume_soft_newline(bytes: &[u8]) -> Option<usize> {
    for &seq in SOFT_NEWLINE_SEQUENCES {
        if bytes.len() >= seq.len() && bytes[..seq.len()] == *seq {
            return Some(seq.len());
        }
    }
    None
}

/// Returns `true` when the buffer starts with the tail of a soft-newline
/// sequence whose leading ESC already sits in the candidate buffer.
fn consume_soft_newline_split(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    // ESC + CR/LF (the common Alt+Enter case).
    if bytes[0] == b'\r' || bytes[0] == b'\n' {
        return true;
    }
    // ESC + `[13;Nu` tail arriving as a second read.
    let tails: &[&[u8]] = &[
        b"[13;2u", b"[13;3u", b"[13;4u", b"[13;5u", b"[13;6u", b"O\r", b"O\n",
    ];
    for tail in tails {
        if bytes.len() >= tail.len() && bytes[..tail.len()] == **tail {
            return true;
        }
    }
    false
}

/// How many bytes of the *new* read to skip after
/// `consume_soft_newline_split` matched.
fn skip_soft_newline_tail(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    if bytes[0] == b'\r' || bytes[0] == b'\n' {
        return 1;
    }
    let tails: &[(&[u8], usize)] = &[
        (b"[13;2u", 6),
        (b"[13;3u", 6),
        (b"[13;4u", 6),
        (b"[13;5u", 6),
        (b"[13;6u", 6),
        (b"O\r", 3),
        (b"O\n", 3),
    ];
    for &(tail, len) in tails {
        if bytes.len() >= tail.len() && bytes[..tail.len()] == *tail {
            return len;
        }
    }
    1
}

fn incomplete_escape_suffix(bytes: &[u8]) -> bool {
    match bytes {
        [0x1b] => true,
        [0x1b, b'[', parameters @ ..] => parameters.iter().all(|byte| matches!(byte, 0x20..=0x3f)),
        [0x1b, b'O'] => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_newline_alt_enter_xterm() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\x1b\rworld");
        assert_eq!(buf.bytes, b"hello\nworld");
        assert!(matches!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending
        ));
    }

    #[test]
    fn soft_newline_shift_enter_modify_other_keys() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\x1b[13;2uworld");
        assert_eq!(buf.bytes, b"hello\nworld");
        assert!(matches!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending
        ));
    }

    #[test]
    fn soft_newline_alt_enter_modify_other_keys() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\x1b[13;3uworld");
        assert_eq!(buf.bytes, b"hello\nworld");
    }

    #[test]
    fn soft_newline_backslash_enter() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\\");
        buf.push(b"\rworld");
        assert_eq!(buf.bytes, b"hello\nworld");
    }

    #[test]
    fn soft_newline_split_read() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\x1b");
        assert!(matches!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending
        ));
        buf.push(b"\rworld");
        assert_eq!(buf.bytes, b"hello\nworld");
    }

    #[test]
    fn bare_cr_submits() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\r");
        assert!(matches!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Complete { .. }
        ));
    }

    #[test]
    fn ctrl_j_is_soft_newline() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\nworld");
        assert!(matches!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending
        ));
        assert_eq!(buf.visible_line_bytes(), b"hello\nworld");
    }

    #[test]
    fn multiline_submit_preserves_newlines() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"line1\nline2\nline3\r");
        if let CandidateLineStatus::Complete { line, .. } = candidate_line_status(&buf.bytes) {
            assert_eq!(line, "line1\nline2\nline3");
        } else {
            panic!("expected Complete");
        }
    }
}
