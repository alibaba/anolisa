use crate::input::InputClassifier;

use super::{CTRL_C, CTRL_U};

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Default)]
pub(super) struct CandidateLineBuffer {
    pub(super) bytes: Vec<u8>,
    /// Offsets in `bytes` holding a literal `\n` inserted for a whitelisted
    /// soft-newline sequence. These offsets never count as a submit
    /// terminator in `candidate_line_status` (#1721).
    pub(super) soft_newlines: Vec<usize>,
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
                byte => {
                    self.bytes.push(byte);
                    self.translate_soft_newline_suffix();
                    idx += 1;
                }
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.bytes.clear();
        self.soft_newlines.clear();
        self.relayed_len = 0;
        self.force_agent_intercept = false;
        self.forced_agent_suggestion_id = None;
    }

    pub(super) fn take(&mut self) -> Vec<u8> {
        self.soft_newlines.clear();
        self.relayed_len = 0;
        self.force_agent_intercept = false;
        self.forced_agent_suggestion_id = None;
        std::mem::take(&mut self.bytes)
    }

    pub(super) fn visible_line_bytes(&self) -> &[u8] {
        let start = self.soft_newlines.last().map_or(0, |offset| offset + 1);
        let end = self.bytes[start..]
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
            .map_or(self.bytes.len(), |offset| start + offset);
        &self.bytes[start..end]
    }

    fn pop_visible_char(&mut self) {
        let end = (0..self.bytes.len())
            .find(|index| {
                matches!(self.bytes[*index], b'\n' | b'\r') && !self.soft_newlines.contains(index)
            })
            .unwrap_or(self.bytes.len());
        if end == 0 {
            return;
        }
        if self.soft_newlines.last() == Some(&(end - 1)) {
            // Undo the soft newline as one unit instead of splitting it.
            self.soft_newlines.pop();
            self.bytes.remove(end - 1);
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

pub(super) fn candidate_line_status(bytes: &[u8], soft_newlines: &[usize]) -> CandidateLineStatus {
    if bytes.len() > 4096 {
        return CandidateLineStatus::Unsafe;
    }

    // Soft newlines are literal \n bytes but never a submit terminator; the
    // first hard \n or \r still submits (bash parity, M1/M2).
    let newline_idx = (0..bytes.len())
        .find(|index| matches!(bytes[*index], b'\n' | b'\r') && !soft_newlines.contains(index));
    let Some(newline_idx) = newline_idx else {
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == 0x1b {
                return if incomplete_escape_suffix(&bytes[index..]) {
                    CandidateLineStatus::Pending
                } else {
                    CandidateLineStatus::Unsafe
                };
            }
            if *byte < 0x20 && !matches!(byte, b'\t') && !soft_newlines.contains(&index) {
                return CandidateLineStatus::Unsafe;
            }
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

    // Cut at the submit terminator instead of trimming trailing \r\n so an
    // inner soft newline right before the terminator survives (I3). For
    // pre-fix shapes this is byte-identical: the line holds no other \n\r.
    let Some(line) = std::str::from_utf8(&bytes[..newline_idx]).ok() else {
        return CandidateLineStatus::Unsafe;
    };
    CandidateLineStatus::Complete {
        line: line.to_string(),
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
// Regression tests for issue #1721 (soft newline in NL prompt input).
// Probe names are kept from the FAIL baseline captured on base commit
// 055e6207 (artifacts/cosh-1721-prompt-soft-newline-baseline), with the
// assertions promoted to the post-fix contract (design.md M3/M5/M6).

#[cfg(test)]
mod probe_1721_tests {
    use super::{candidate_line_status, CandidateLineBuffer, CandidateLineStatus};

    fn buffer_after_push(inputs: &[&[u8]]) -> CandidateLineBuffer {
        let mut buffer = CandidateLineBuffer::default();
        for input in inputs {
            buffer.push(input);
        }
        buffer
    }

    fn status_of(buffer: &CandidateLineBuffer) -> CandidateLineStatus {
        candidate_line_status(&buffer.bytes, &buffer.soft_newlines)
    }

    #[test]
    fn probe_1721_alt_enter_soft_newline() {
        // Alt+Enter in xterm arrives as ESC + CR (M3): translated to a
        // literal newline in the buffer, staying Pending instead of being
        // flushed as Unsafe or submitted as Complete.
        let buffer = buffer_after_push(&[b"hello\x1b\r"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
        assert_eq!(buffer.bytes, b"hello\n");
    }

    #[test]
    fn probe_1721_shift_enter_csi_u_soft_newline() {
        // Shift+Enter under CSI-u (kitty keyboard protocol): ESC [ 27 ; 2 u (M5).
        let buffer = buffer_after_push(&[b"hello\x1b[27;2u"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
        assert_eq!(buffer.bytes, b"hello\n");
    }

    #[test]
    fn probe_1721_shift_enter_csi_u13_soft_newline() {
        // Shift+Enter CSI-u variant: ESC [ 13 ; 2 u (M6).
        let buffer = buffer_after_push(&[b"hello\x1b[13;2u"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
        assert_eq!(buffer.bytes, b"hello\n");
    }

    #[test]
    fn probe_1721_ctrl_j_submits_like_enter() {
        // Status-quo guard (bash parity, M2): Ctrl+J (0x0a) is treated exactly
        // like Enter and submits the line immediately, as reported in #1721.
        let buffer = buffer_after_push(&[b"hello\x0a"]);
        match status_of(&buffer) {
            CandidateLineStatus::Complete { line, line_len } => {
                assert_eq!(line, "hello");
                assert_eq!(line_len, 6);
            }
            other => panic!("Ctrl+J should submit like Enter, got {other:?}"),
        }
    }
}
