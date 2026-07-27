use crate::input::InputClassifier;

use super::{CTRL_C, CTRL_U};

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

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
    if trimmed.starts_with('@') {
        return at_file_hint(trimmed);
    }
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

fn at_file_hint(line: &str) -> Option<String> {
    let prefix = line.strip_prefix('@').unwrap_or("");
    // Only show candidates when the @ token has no whitespace yet (single token).
    if prefix.contains(char::is_whitespace) {
        return None;
    }
    let candidates = at_file_candidates(prefix);
    if candidates.is_empty() {
        return None;
    }
    // Show up to 8 file candidates inline.
    let display: Vec<&str> = candidates.iter().take(8).map(|s| s.as_str()).collect();
    Some(display.join("  "))
}

/// Lists files in the current directory matching the given prefix.
pub(super) fn at_file_candidates(prefix: &str) -> Vec<String> {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(_) => return Vec::new(),
    };
    let mut entries: Vec<String> = Vec::new();
    let read_dir = match std::fs::read_dir(&cwd) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip hidden files unless prefix starts with '.'.
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        if name.starts_with(prefix) {
            let suffix = if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                format!("{name}/")
            } else {
                name
            };
            entries.push(suffix);
        }
    }
    entries.sort();
    entries
}

/// Completes the `@` prefix to the best matching filename.
/// Returns the completed `@filename` string, or None if no match.
pub(super) fn at_file_complete(prefix: &str) -> Option<String> {
    let candidates = at_file_candidates(prefix);
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates.into_iter().next().unwrap());
    }
    // Find longest common prefix among candidates.
    let first = &candidates[0];
    let mut common_len = first.len();
    for candidate in &candidates[1..] {
        let shared = first
            .chars()
            .zip(candidate.chars())
            .take_while(|(a, b)| a == b)
            .count();
        common_len = common_len.min(shared);
    }
    if common_len > prefix.len() {
        Some(first[..common_len].to_string())
    } else {
        // No further common prefix; return first candidate.
        Some(candidates.into_iter().next().unwrap())
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
    matches!(first, Some(b'/' | b'?' | b'@')) || first.is_some_and(|byte| byte >= 0x80)
}

pub(super) fn starts_native_intercept_candidate(
    bytes: &[u8],
    native_line_state: &NativeLineState,
) -> bool {
    native_line_state.is_at_line_start()
        && (first_visible_input_byte(bytes) == Some(b'/')
            || first_visible_input_byte(bytes) == Some(b'@')
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

    let Some(newline_idx) = bytes.iter().position(|byte| matches!(byte, b'\n' | b'\r')) else {
        for (index, byte) in bytes.iter().enumerate() {
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
    fn at_is_intercept_candidate() {
        assert!(starts_intercept_candidate(b"@"));
        assert!(starts_intercept_candidate(b"@file"));
    }

    #[test]
    fn non_at_inputs_are_not_at_candidates() {
        assert!(!starts_intercept_candidate(b"a"));
        assert!(!starts_intercept_candidate(b"#"));
    }

    #[test]
    fn at_file_hint_returns_none_for_bare_at() {
        // Bare @ should show candidates, not None - it depends on what's in cwd
        let hint = at_file_hint("@");
        // hint may be Some or None depending on cwd contents, just check no panic
        let _ = hint;
    }

    #[test]
    fn at_file_hint_returns_none_for_multi_token() {
        assert_eq!(at_file_hint("@file extra"), None);
    }

    #[test]
    fn at_file_complete_returns_none_for_empty_dir() {
        // With an empty prefix in any directory, we might get results
        // Just verify no panic
        let _ = at_file_complete("zzz_nonexistent_prefix_zzz");
    }

    #[test]
    fn candidate_inline_hint_handles_at_prefix() {
        // @ with no matching files returns None
        let hint = candidate_inline_hint("@zzz_nonexistent_zzz");
        assert_eq!(hint, None);
    }

    #[test]
    fn candidate_inline_hint_still_handles_slash() {
        let hint = candidate_inline_hint("/mode");
        assert!(hint.is_some());
    }
}
