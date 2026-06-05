use std::borrow::Cow;

pub(super) fn strip_replayed_prompt_prefix<'a>(
    bytes: &'a [u8],
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
) -> &'a [u8] {
    let Some(raw_prompt) = replayed_prompt_prefix.as_deref() else {
        return bytes;
    };

    let replay_prompt = prompt_replay_bytes(raw_prompt);
    let replay_start = leading_replay_separator_len(bytes);
    let replay_bytes = &bytes[replay_start..];
    if !bytes.is_empty() && replay_bytes.is_empty() {
        return replay_bytes;
    }
    let stripped = if replay_bytes.starts_with(raw_prompt) {
        Some(&replay_bytes[raw_prompt.len()..])
    } else if replay_prompt.len() != raw_prompt.len() && replay_bytes.starts_with(replay_prompt) {
        Some(&replay_bytes[replay_prompt.len()..])
    } else {
        None
    };

    if let Some(rest) = stripped {
        if !rest.is_empty() && leading_replay_separator_len(rest) == rest.len() {
            return &rest[rest.len()..];
        }
    }

    if !bytes.is_empty() || stripped.is_some() {
        *replayed_prompt_prefix = None;
    }
    stripped.unwrap_or(bytes)
}

fn leading_replay_separator_len(bytes: &[u8]) -> usize {
    let mut idx = 0;
    while idx < bytes.len() {
        if matches!(bytes[idx], b'\r' | b'\n') {
            idx += 1;
            continue;
        }
        if bytes[idx..].starts_with(b"\x1b[?2004h") || bytes[idx..].starts_with(b"\x1b[?2004l") {
            idx += b"\x1b[?2004h".len();
            continue;
        }
        break;
    }
    idx
}

pub(super) fn prompt_replay_bytes(prompt: &[u8]) -> &[u8] {
    strip_zsh_partial_line_marker(prompt).unwrap_or(prompt)
}

pub(super) fn prompt_prefixed_replay_bytes<'a>(bytes: &'a [u8], prompt: &'a [u8]) -> Cow<'a, [u8]> {
    if prompt.is_empty() || !bytes.starts_with(prompt) {
        return Cow::Borrowed(bytes);
    }

    let replay = prompt_replay_bytes(prompt);
    if replay.len() == prompt.len() {
        return Cow::Borrowed(bytes);
    }

    let mut replayed = Vec::with_capacity(replay.len() + bytes.len().saturating_sub(prompt.len()));
    replayed.extend_from_slice(replay);
    replayed.extend_from_slice(&bytes[prompt.len()..]);
    Cow::Owned(replayed)
}

fn strip_zsh_partial_line_marker(prompt: &[u8]) -> Option<&[u8]> {
    let marker_end = prompt.iter().position(|byte| *byte == b'\n')?;
    if marker_end > 512 {
        return None;
    }
    if !visible_line_is_zsh_partial_marker(&prompt[..marker_end]) {
        return None;
    }
    let after_newline = marker_end + 1;
    if prompt[after_newline..].starts_with(b"\x1b[A") {
        Some(&prompt[marker_end..])
    } else {
        Some(&prompt[after_newline..])
    }
}

fn visible_line_is_zsh_partial_marker(line: &[u8]) -> bool {
    let mut visible = Vec::new();
    let mut idx = 0;
    while idx < line.len() {
        match line[idx] {
            b'\x1b' if line.get(idx + 1) == Some(&b'[') => {
                idx += 2;
                while idx < line.len() {
                    let byte = line[idx];
                    idx += 1;
                    if byte.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            b'\r' => idx += 1,
            b'\x08' => {
                visible.pop();
                idx += 1;
            }
            byte => {
                visible.push(byte);
                idx += 1;
            }
        }
    }

    visible
        .iter()
        .all(|byte| byte.is_ascii_whitespace() || *byte == b'%')
        && visible.contains(&b'%')
        && visible.iter().filter(|byte| **byte == b'%').count() == 1
}

#[cfg(test)]
mod tests {
    use super::{prompt_prefixed_replay_bytes, prompt_replay_bytes, strip_replayed_prompt_prefix};

    #[test]
    fn prompt_replay_strips_zsh_partial_line_marker() {
        let prompt = b"\x1b[0m\x1b[1m\x1b[7m%\x1b[27m\x1b[0m      \r\x1b[K\r\r\n\x1b[Aprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), b"\n\x1b[Aprompt> ");
    }

    #[test]
    fn prompt_replay_strips_plain_zsh_percent_marker_line() {
        let prompt = b"%\r\nprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), b"prompt> ");
    }

    #[test]
    fn prompt_replay_strips_styled_plain_percent_marker_line() {
        let prompt = b"\x1b[1m%\x1b[0m   \r\x1b[K\nprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), b"prompt> ");
    }

    #[test]
    fn prompt_replay_keeps_literal_percent_prompt() {
        let prompt = b"usage 50% prompt> ";

        assert_eq!(prompt_replay_bytes(prompt), prompt);
    }

    #[test]
    fn prompt_replay_keeps_multiline_prompt_with_non_marker_percent() {
        let prompt = b"usage 50%\nprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), prompt);
    }

    #[test]
    fn prompt_prefixed_replay_strips_marker_when_releasing_held_prompt() {
        let prompt = b"\x1b[1m%\x1b[0m   \r\x1b[K\nprompt> ";
        let display = b"\x1b[1m%\x1b[0m   \r\x1b[K\nprompt> echo after\r\n";

        assert_eq!(
            prompt_prefixed_replay_bytes(display, prompt).as_ref(),
            b"prompt> echo after\r\n"
        );
    }

    #[test]
    fn prompt_prefixed_replay_keeps_non_prompt_output() {
        let prompt = b"prompt> ";
        let display = b"%\r\nregular command output\r\n";

        assert_eq!(
            prompt_prefixed_replay_bytes(display, prompt).as_ref(),
            display
        );
    }

    #[test]
    fn replayed_prompt_prefix_is_suppressed_from_next_pty_echo() {
        let mut replayed = Some(b"prompt> ".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> echo after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_tolerates_leading_newline() {
        let mut replayed = Some(b"prompt> ".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"\r\nprompt> echo after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_tolerates_bracketed_paste_toggle() {
        let mut replayed = Some(b"prompt> \x1b[?2004h".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(
                b"\x1b[?2004l\r\nprompt> \x1b[?2004hecho after\r\n",
                &mut replayed
            ),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_keeps_pending_after_control_only_slice() {
        let mut replayed = Some(b"prompt> \x1b[?2004h".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"\x1b[?2004l\r\n", &mut replayed),
            b""
        );
        assert!(replayed.is_some());
        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> \x1b[?2004hecho after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_keeps_pending_after_prompt_control_only_slice() {
        let mut replayed = Some(b"prompt> \x1b[?2004h".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> \x1b[?2004h\x1b[?2004l\r\n", &mut replayed),
            b""
        );
        assert!(replayed.is_some());
        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> \x1b[?2004hecho after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }
}
