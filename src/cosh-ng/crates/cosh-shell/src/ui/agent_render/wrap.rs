use ratatui::text::{Line, Span};

pub(super) fn line_is_empty(line: &Line<'static>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

pub(super) fn line_to_string(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("")
}

pub(super) fn ordered_list_item(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let marker_end = trimmed.find(". ")?;
    if marker_end == 0 || !trimmed[..marker_end].chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some((&trimmed[..marker_end + 2], &trimmed[marker_end + 2..]))
}

pub(crate) fn wrap_plain_line(line: &str, width: usize) -> Vec<String> {
    if line.trim().is_empty() {
        return vec![String::new()];
    }

    let (first_prefix, rest_prefix, text) = split_prefix(line);
    wrap_with_prefix(text, &first_prefix, &rest_prefix, width)
}

pub(crate) fn wrap_plain_line_with_prefix(
    text: &str,
    first_prefix: &str,
    rest_prefix: &str,
    width: usize,
) -> Vec<String> {
    wrap_with_prefix(text, first_prefix, rest_prefix, width)
}

pub(super) fn compact_rendered_lines(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect()
}

fn split_prefix(line: &str) -> (String, String, &str) {
    if let Some((indent, marker, rest)) = list_item_prefix(line) {
        let first_prefix = format!("{indent}{marker}");
        let rest_prefix = format!("{indent}{}", " ".repeat(marker.len()));
        (first_prefix, rest_prefix, rest)
    } else if let Some(rest) = line.strip_prefix("> ") {
        ("> ".to_string(), "  ".to_string(), rest)
    } else if line.starts_with("  ") {
        ("  ".to_string(), "  ".to_string(), line.trim_start())
    } else {
        ("".to_string(), "".to_string(), line.trim_start())
    }
}

fn list_item_prefix(line: &str) -> Option<(&str, &str, &str)> {
    let indent_len = line
        .char_indices()
        .take_while(|(_, ch)| *ch == ' ')
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()
        .unwrap_or(0);
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    if let Some(item) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        return Some((indent, "- ", item));
    }
    let (marker, item) = ordered_list_item(rest)?;
    Some((indent, marker, item))
}

fn wrap_with_prefix(
    text: &str,
    first_prefix: &str,
    rest_prefix: &str,
    width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut prefix = first_prefix;
    let mut current = String::new();
    let mut current_width = display_width(prefix);
    let max_width = width.max(current_width + 1);

    current.push_str(prefix);
    for mut token in split_wrap_tokens(text) {
        if token == "\n" {
            lines.push(current.trim_end().to_string());
            prefix = rest_prefix;
            current.clear();
            current.push_str(prefix);
            current_width = display_width(prefix);
            continue;
        }

        if current.trim() == prefix.trim() {
            token = token.trim_start().to_string();
        }

        let token_width = display_width(&token);
        if token_width > 0 && current_width + token_width > max_width {
            if current.trim() != prefix.trim() {
                lines.push(current.trim_end().to_string());
                prefix = rest_prefix;
                current.clear();
                current.push_str(prefix);
                current_width = display_width(prefix);
                token = token.trim_start().to_string();
            }

            if display_width(&token) + current_width > max_width {
                let wrapped =
                    wrap_long_token(&token, rest_prefix, max_width, current, current_width);
                lines.extend(wrapped.finished_lines);
                current = wrapped.current_line;
                current_width = wrapped.current_width;
                prefix = rest_prefix;
                continue;
            }
        }

        current.push_str(&token);
        current_width += display_width(&token);
    }

    if !current.trim().is_empty() || lines.is_empty() {
        lines.push(current.trim_end().to_string());
    }
    lines
}

fn split_wrap_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut last_was_space = None;

    for ch in text.chars() {
        if ch == '\n' {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            tokens.push("\n".to_string());
            last_was_space = None;
            continue;
        }

        let is_space = ch.is_whitespace();
        if last_was_space.is_some_and(|was_space| was_space != is_space) && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        current.push(ch);
        last_was_space = Some(is_space);
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

struct WrappedToken {
    finished_lines: Vec<String>,
    current_line: String,
    current_width: usize,
}

fn wrap_long_token(
    token: &str,
    rest_prefix: &str,
    max_width: usize,
    mut current: String,
    mut current_width: usize,
) -> WrappedToken {
    let mut finished_lines = Vec::new();

    for ch in token.chars() {
        let ch_width = char_width(ch);
        if ch_width > 0 && current_width + ch_width > max_width {
            finished_lines.push(current.trim_end().to_string());
            current.clear();
            current.push_str(rest_prefix);
            current_width = display_width(rest_prefix);
        }
        current.push(ch);
        current_width += ch_width;
    }

    WrappedToken {
        finished_lines,
        current_line: current,
        current_width,
    }
}

pub(super) fn strip_ansi_escape(text: &str) -> String {
    let mut stripped = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            stripped.push(ch);
            continue;
        }

        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    stripped
}

pub(crate) fn display_width(text: &str) -> usize {
    Span::raw(strip_ansi_escape(text)).width()
}

pub(super) fn char_width(ch: char) -> usize {
    let mut text = [0; 4];
    display_width(ch.encode_utf8(&mut text))
}

pub(super) fn should_buffer_word_char(ch: char) -> bool {
    ch.is_ascii() && !ch.is_whitespace() && !ch.is_control()
}

pub(super) fn is_line_closing_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '。' | '，' | '、' | '；' | '：' | '！' | '？' | ')' | ']' | '}' | '）' | '】' | '》'
    )
}
