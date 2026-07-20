use std::path::Path;

use crate::evidence::model::OutputExcerptDirection;
use crate::evidence::redact_sensitive_text;

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

/// Applies the shared output policy before content crosses a durable or provider boundary.
pub(crate) fn redact_sensitive_output(text: &str) -> (String, bool) {
    let (redacted, changed, _) = redact_sensitive_output_with_policy(text);
    (redacted, changed)
}

pub(super) fn redact_sensitive_output_with_policy(text: &str) -> (String, bool, bool) {
    let (redacted, home_changed) = redact_home_path(text);
    let (redacted, secret_changed) = redact_sensitive_text(&redacted);
    (redacted, home_changed || secret_changed, secret_changed)
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
