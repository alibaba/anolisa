use crate::input::InputClassifier;

use super::{CTRL_C, CTRL_U};

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

/// Sentinel byte stored in `CandidateLineBuffer::bytes` to represent a
/// soft newline inserted via Alt+Enter or Shift+Enter.  The byte must
/// not appear naturally in terminal input and must be converted back to
/// `\n` before the completed line is delivered to the agent or shell.
const SOFT_NEWLINE_BYTE: u8 = 0x1e;

/// Terminal escape sequences produced by Alt+Enter / Shift+Enter that
/// should be interpreted as a soft newline rather than a submission.
const SOFT_NEWLINE_SEQUENCES: &[&[u8]] = &[
    b"\x1b\r",     // Alt+Enter (most terminals)
    b"\x1b\n",     // Alt+Enter (some terminals)
    b"\x1b[27;2u",  // Shift+Enter (xterm modifyOtherKeys, keyCode 27=Esc with shift)
    b"\x1b[13;2u",  // Shift+Enter (xterm modifyOtherKeys, keyCode 13=CR with shift)
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
        // Handle a soft-newline sequence that was split across two push()
        // calls: if self.bytes ends with a partial soft-newline prefix
        // (e.g. lone 0x1b) and the new bytes complete the sequence.
        if !bytes.is_empty() {
            if let Some(consumed) = self.try_complete_split_soft_newline(bytes) {
                idx = consumed;
            }
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
            // Detect soft-newline escape sequences (Alt+Enter, Shift+Enter)
            // and insert a sentinel byte instead of triggering submission.
            if let Some(seq) = soft_newline_sequence_at(&bytes[idx..]) {
                self.bytes.push(SOFT_NEWLINE_BYTE);
                idx += seq.len();
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

    /// Checks whether the tail of `self.bytes` plus a prefix of `incoming`
    /// forms a complete soft-newline escape sequence.  If so, removes the
    /// partial prefix from `self.bytes`, pushes `SOFT_NEWLINE_BYTE`, and
    /// returns how many bytes from `incoming` were consumed.
    fn try_complete_split_soft_newline(&mut self, incoming: &[u8]) -> Option<usize> {
        // Only the lone 0x1b split is relevant (all soft-newline sequences
        // start with 0x1b).
        if self.bytes.last() != Some(&0x1b) {
            return None;
        }
        // Build a small combined view: last byte + incoming prefix.
        let max_seq_len = SOFT_NEWLINE_SEQUENCES
            .iter()
            .map(|s| s.len())
            .max()
            .unwrap_or(0);
        let need = max_seq_len.saturating_sub(1).min(incoming.len());
        let mut combined = Vec::with_capacity(1 + need);
        combined.push(0x1b);
        combined.extend_from_slice(&incoming[..need]);
        let seq = soft_newline_sequence_at(&combined)?;
        let consumed = seq.len() - 1; // bytes taken from `incoming`
        self.bytes.pop(); // remove the trailing 0x1b
        self.bytes.push(SOFT_NEWLINE_BYTE);
        Some(consumed)
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
        // Soft-newline sentinel bytes are part of the visible multiline
        // content; only bare \n/\r mark the submission boundary.
        let end = self
            .bytes
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
            .unwrap_or(self.bytes.len());
        &self.bytes[..end]
    }

    /// Returns `visible_line_bytes()` with soft-newline sentinels
    /// replaced by literal `\n` for terminal rendering.
    pub(super) fn visible_line_render_bytes(&self) -> Vec<u8> {
        self.visible_line_bytes()
            .iter()
            .map(|&b| if b == SOFT_NEWLINE_BYTE { b'\n' } else { b })
            .collect()
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

pub(super) fn candidate_line_status(bytes: &[u8]) -> CandidateLineStatus {
    if bytes.len() > 4096 {
        return CandidateLineStatus::Unsafe;
    }

    // Find the first bare \n or \r (submission trigger).
    // Soft-newline sentinel bytes (SOFT_NEWLINE_BYTE) are NOT submission triggers.
    let Some(newline_idx) = bytes
        .iter()
        .position(|byte| matches!(byte, b'\n' | b'\r'))
    else {
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == SOFT_NEWLINE_BYTE {
                continue;
            }
            if *byte == 0x1b {
                return if incomplete_escape_suffix(&bytes[index..]) {
                    CandidateLineStatus::Pending
                } else {
                    CandidateLineStatus::Unsafe
                };
            }
            if *byte < 0x20 && !matches!(byte, b'\t') {
                return CandidateLineStatus::Unsafe;
            }
        }
        return CandidateLineStatus::Pending;
    };

    let line_len = newline_idx + 1;
    let line_bytes = &bytes[..line_len];
    if line_bytes.iter().any(|byte| {
        *byte != SOFT_NEWLINE_BYTE
            && (*byte == 0x1b
                || (*byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t')))
    }) {
        return CandidateLineStatus::Unsafe;
    }

    let Some(line) = std::str::from_utf8(line_bytes).ok() else {
        return CandidateLineStatus::Unsafe;
    };
    // Convert soft-newline sentinels back to literal newlines before delivery.
    let line = line
        .trim_end_matches(['\r', '\n'])
        .replace(SOFT_NEWLINE_BYTE as char, "\n");
    CandidateLineStatus::Complete { line, line_len }
}

fn incomplete_escape_suffix(bytes: &[u8]) -> bool {
    match bytes {
        [0x1b] => true,
        [0x1b, b'[', parameters @ ..] => parameters.iter().all(|byte| matches!(byte, 0x20..=0x3f)),
        [0x1b, b'O'] => true,
        _ => false,
    }
}

/// If `bytes` starts with a recognized soft-newline escape sequence
/// (Alt+Enter or Shift+Enter), returns that sequence slice.
fn soft_newline_sequence_at(bytes: &[u8]) -> Option<&'static [u8]> {
    for &seq in SOFT_NEWLINE_SEQUENCES {
        if bytes.len() >= seq.len() && bytes[..seq.len()] == *seq {
            return Some(seq);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_enter_produces_soft_newline_in_candidate_buffer() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello");
        buf.push(b"\x1b\r"); // Alt+Enter
        buf.push(b"world");
        assert_eq!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending,
            "soft newline must not trigger submission",
        );
        // Bare Enter should now complete the line with embedded newlines.
        buf.push(b"\r");
        match candidate_line_status(&buf.bytes) {
            CandidateLineStatus::Complete { line, .. } => {
                assert_eq!(line, "hello\nworld");
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn alt_enter_lf_produces_soft_newline() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"line1\x1b\nline2");
        assert_eq!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending,
        );
    }

    #[test]
    fn shift_enter_modify_other_keys_produces_soft_newline() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"aaa\x1b[27;2ubbb");
        assert_eq!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending,
        );
        buf.push(b"\r");
        match candidate_line_status(&buf.bytes) {
            CandidateLineStatus::Complete { line, .. } => {
                assert_eq!(line, "aaa\nbbb");
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn shift_enter_cr_variant_produces_soft_newline() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"x\x1b[13;2uy");
        assert_eq!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending,
        );
    }

    #[test]
    fn split_alt_enter_across_pushes_produces_soft_newline() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\x1b");
        // At this point the lone ESC makes the status Pending.
        assert_eq!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending,
        );
        buf.push(b"\r"); // completes Alt+Enter across push boundary
        buf.push(b"world");
        assert_eq!(
            candidate_line_status(&buf.bytes),
            CandidateLineStatus::Pending,
        );
    }

    #[test]
    fn multiple_soft_newlines_produce_multiline_submission() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"line1\x1b\rline2\x1b\rline3\r");
        match candidate_line_status(&buf.bytes) {
            CandidateLineStatus::Complete { line, .. } => {
                assert_eq!(line, "line1\nline2\nline3");
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn visible_line_render_bytes_converts_soft_newlines() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"aaa\x1b\rbbb");
        let rendered = buf.visible_line_render_bytes();
        assert_eq!(rendered, b"aaa\nbbb");
    }

    #[test]
    fn pop_visible_char_handles_soft_newline() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\x1b\r"); // "hello" + soft newline
        // Pop should remove the soft newline sentinel
        buf.pop_visible_char();
        assert_eq!(buf.visible_line_render_bytes(), b"hello");
        // Pop the 'o'
        buf.pop_visible_char();
        assert_eq!(buf.visible_line_render_bytes(), b"hell");
    }

    #[test]
    fn bare_enter_still_submits_immediately() {
        let mut buf = CandidateLineBuffer::default();
        buf.push(b"hello\r");
        match candidate_line_status(&buf.bytes) {
            CandidateLineStatus::Complete { line, .. } => {
                assert_eq!(line, "hello");
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }
}
