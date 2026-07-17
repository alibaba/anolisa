use std::path::Path;

use crate::evidence::model::OutputExcerptDirection;

pub(super) const PROVIDER_PREVIEW_MAX_CHARS: usize = 6_000;
const PROVIDER_PREVIEW_HEAD_CHARS: usize = 4_000;
const PROVIDER_PREVIEW_TAIL_CHARS: usize = 1_500;

pub(super) struct ProviderOutputPreview {
    pub(super) text: Option<String>,
    pub(super) redaction_status: &'static str,
    pub(super) reason: &'static str,
    pub(super) truncated: bool,
    pub(super) complete: bool,
}

pub(super) fn provider_output_preview(
    output_ref: Option<&str>,
    output_id: &str,
) -> ProviderOutputPreview {
    let Some(output_ref) = output_ref else {
        return ProviderOutputPreview {
            text: None,
            redaction_status: "preview_unavailable",
            reason: "<none>",
            truncated: false,
            complete: false,
        };
    };
    let Ok(text) = std::fs::read_to_string(Path::new(output_ref)) else {
        return ProviderOutputPreview {
            text: None,
            redaction_status: "preview_unavailable",
            reason: "<unavailable>",
            truncated: false,
            complete: false,
        };
    };

    let text = clean_terminal_control_sequences(&text);
    let (redacted, found_sensitive) = redact_sensitive_output(&text);
    let (bounded, truncated) = truncate_preview(&redacted, PROVIDER_PREVIEW_MAX_CHARS, output_id);
    let redaction_status = if found_sensitive || truncated {
        "preview_redacted"
    } else {
        "preview_included"
    };

    ProviderOutputPreview {
        text: Some(bounded),
        redaction_status,
        reason: "<preview omitted>",
        truncated,
        complete: !truncated,
    }
}

pub(super) fn redact_sensitive_output(text: &str) -> (String, bool) {
    let (redacted, changed, _) = redact_sensitive_output_with_policy(text);
    (redacted, changed)
}

pub(super) fn redact_sensitive_output_with_policy(text: &str) -> (String, bool, bool) {
    let (redacted, home_changed) = redact_home_path(text);
    let (redacted, private_key_changed) = redact_private_key_blocks(&redacted);
    let mut changed = home_changed || private_key_changed;
    let mut confirmation_required = private_key_changed;

    let mut lines = Vec::new();
    for line in redacted.lines() {
        let (line, line_changed) = redact_sensitive_line(line);
        changed |= line_changed;
        confirmation_required |= line_changed;
        lines.push(line);
    }
    let mut output = lines.join("\n");
    if text.ends_with('\n') {
        output.push('\n');
    }
    (output, changed, confirmation_required)
}

fn redact_private_key_blocks(text: &str) -> (String, bool) {
    const BEGIN: &str = "-----BEGIN ";
    const END: &str = "-----END ";
    const SUFFIX: &str = "PRIVATE KEY-----";

    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut changed = false;
    while let Some(relative_begin) = text[cursor..].find(BEGIN) {
        let begin = cursor + relative_begin;
        let marker_tail = &text[begin + BEGIN.len()..];
        let Some(relative_begin_end) = marker_tail.find(SUFFIX) else {
            break;
        };
        let begin_end = begin + BEGIN.len() + relative_begin_end + SUFFIX.len();
        if text[begin..begin_end].contains('\n') {
            output.push_str(&text[cursor..begin + BEGIN.len()]);
            cursor = begin + BEGIN.len();
            continue;
        }

        output.push_str(&text[cursor..begin]);
        output.push_str("<redacted private key block>");
        changed = true;
        let after_begin = &text[begin_end..];
        let Some(relative_end) = after_begin.find(END) else {
            cursor = text.len();
            break;
        };
        let end = begin_end + relative_end;
        let end_tail = &text[end + END.len()..];
        let Some(relative_end_marker) = end_tail.find(SUFFIX) else {
            cursor = text.len();
            break;
        };
        let end_marker_end = end + END.len() + relative_end_marker + SUFFIX.len();
        if text[end..end_marker_end].contains('\n') {
            cursor = text.len();
            break;
        }
        cursor = end_marker_end;
    }
    output.push_str(&text[cursor..]);
    (output, changed)
}

pub(super) fn clean_terminal_control_sequences(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if ch == '\r' {
            continue;
        }
        if ch.is_control() && !matches!(ch, '\n' | '\t') {
            continue;
        }
        output.push(ch);
    }
    output
}

fn redact_home_path(text: &str) -> (String, bool) {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() && text.contains(&home) {
            return (text.replace(&home, "~"), true);
        }
    }
    (text.to_string(), false)
}

fn redact_sensitive_line(line: &str) -> (String, bool) {
    if line.contains("PRIVATE KEY-----") {
        return ("<redacted private key marker>".to_string(), true);
    }

    let (line, bearer_changed) = redact_bearer_token(line);
    let (line, aws_changed) = redact_aws_access_key(&line);
    (line, bearer_changed || aws_changed)
}

fn redact_bearer_token(line: &str) -> (String, bool) {
    let lower = line.to_ascii_lowercase();
    let Some(start) = lower.find("bearer ") else {
        return (line.to_string(), false);
    };
    let token_start = start + "bearer ".len();
    let token_end = line[token_start..]
        .find(char::is_whitespace)
        .map(|idx| token_start + idx)
        .unwrap_or(line.len());
    let mut redacted = String::new();
    redacted.push_str(&line[..token_start]);
    redacted.push_str("<redacted>");
    redacted.push_str(&line[token_end..]);
    (redacted, true)
}

fn redact_aws_access_key(line: &str) -> (String, bool) {
    let mut output = String::new();
    let mut token = String::new();
    let mut changed = false;

    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() {
            token.push(ch);
            continue;
        }
        push_redacted_token(&mut output, &token, &mut changed);
        token.clear();
        output.push(ch);
    }
    push_redacted_token(&mut output, &token, &mut changed);
    (output, changed)
}

fn push_redacted_token(output: &mut String, token: &str, changed: &mut bool) {
    if token.starts_with("AKIA")
        && token.len() >= 20
        && token
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
    {
        output.push_str("AKIA<redacted>");
        *changed = true;
    } else {
        output.push_str(token);
    }
}

fn truncate_preview(value: &str, max_chars: usize, output_id: &str) -> (String, bool) {
    let total_chars = value.chars().count();
    if total_chars <= max_chars {
        return (value.to_string(), false);
    }

    let full_marker = format!(
        "\n\n... <truncated; for more output use cosh_shell_evidence action=read_output output_id={output_id} direction=tail lines=300>\n\n"
    );
    let marker = if full_marker.chars().count() < max_chars {
        full_marker
    } else {
        "\n\n... <truncated; for more output use cosh_shell_evidence read_output with output_id from metadata>\n\n".to_string()
    };
    let marker_chars = marker.chars().count();
    let available_chars = max_chars.saturating_sub(marker_chars);
    let head_chars = PROVIDER_PREVIEW_HEAD_CHARS.min(available_chars);
    let tail_chars = PROVIDER_PREVIEW_TAIL_CHARS.min(available_chars.saturating_sub(head_chars));
    let head = value.chars().take(head_chars).collect::<String>();
    let tail = value
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();

    (format!("{head}{marker}{tail}"), true)
}

pub(super) fn select_output_lines(
    text: &str,
    direction: OutputExcerptDirection,
    max_lines: usize,
) -> (String, bool) {
    let lines = text.lines().collect::<Vec<_>>();
    let truncated = lines.len() > max_lines;
    let selected = match direction {
        OutputExcerptDirection::Head => lines.iter().take(max_lines).copied().collect::<Vec<_>>(),
        OutputExcerptDirection::Tail => lines
            .iter()
            .rev()
            .take(max_lines)
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>(),
    };
    let mut output = selected.join("\n");
    if text.ends_with('\n') && selected.len() == lines.len() {
        output.push('\n');
    }
    (output, truncated)
}

pub(super) fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }

    const MARKER: &str = "... <truncated>";
    if max_bytes <= MARKER.len() {
        return (MARKER[..max_bytes].to_string(), true);
    }

    let mut end = (max_bytes - MARKER.len()).min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}{MARKER}", &value[..end]), true)
}
