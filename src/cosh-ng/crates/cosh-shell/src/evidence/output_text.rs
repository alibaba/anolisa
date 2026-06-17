use std::path::Path;

use crate::evidence::model::OutputExcerptDirection;

pub(super) const PROVIDER_PREVIEW_MAX_CHARS: usize = 2_000;

pub(super) struct ProviderOutputPreview {
    pub(super) text: Option<String>,
    pub(super) redaction_status: &'static str,
    pub(super) reason: &'static str,
}

pub(super) fn provider_output_preview(output_ref: Option<&str>) -> ProviderOutputPreview {
    let Some(output_ref) = output_ref else {
        return ProviderOutputPreview {
            text: None,
            redaction_status: "preview_unavailable",
            reason: "<none>",
        };
    };
    let Ok(text) = std::fs::read_to_string(Path::new(output_ref)) else {
        return ProviderOutputPreview {
            text: None,
            redaction_status: "preview_unavailable",
            reason: "<unavailable>",
        };
    };

    let text = clean_terminal_control_sequences(&text);
    let (redacted, found_sensitive) = redact_sensitive_output(&text);
    let (bounded, truncated) = truncate_preview(&redacted, PROVIDER_PREVIEW_MAX_CHARS);
    let redaction_status = if found_sensitive || truncated {
        "preview_redacted"
    } else {
        "preview_included"
    };

    ProviderOutputPreview {
        text: Some(bounded),
        redaction_status,
        reason: "<preview omitted>",
    }
}

pub(super) fn redact_sensitive_output(text: &str) -> (String, bool) {
    let (redacted, mut changed) = redact_home_path(text);

    let mut lines = Vec::new();
    for line in redacted.lines() {
        let (line, line_changed) = redact_sensitive_line(line);
        changed |= line_changed;
        lines.push(line);
    }
    let mut output = lines.join("\n");
    if text.ends_with('\n') {
        output.push('\n');
    }
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

fn truncate_preview(value: &str, max_chars: usize) -> (String, bool) {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        (format!("{truncated}... <truncated>"), true)
    } else {
        (truncated, false)
    }
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

    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}... <truncated>", &value[..end]), true)
}
