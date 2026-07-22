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

/// Canonicalizes untrusted terminal output for display and redaction.
///
/// Removes, as *complete sequences* (never just the introducer):
/// - CSI (`ESC [` and C1 `U+009B`) up to and including the final byte;
/// - OSC (`ESC ]` / C1 `U+009D`) up to BEL or ST;
/// - DCS / SOS / PM / APC (`ESC P`/`X`/`^`/`_` and C1 `U+0090`/`U+0098`/
///   `U+009E`/`U+009F`) up to ST — their payload never reaches the output;
/// - `\r`, bare ESC, and every other control character except `\n`/`\t`;
/// - invisible Unicode format / default-ignorable characters (zero-width
///   characters, bidi controls, variation selectors, tags), which can
///   visually spoof output or split sensitive keywords so redaction never
///   sees their canonical form.
///
/// Callers that feed the result into secret redaction must run this FIRST:
/// removing invisible characters after redaction lets an attacker split a
/// sensitive key (`api_\0key=...`, `api_\u{200B}key=...`) so the patterns
/// never match while the terminal still shows an ordinary assignment.
pub(crate) fn clean_terminal_control_sequences(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    consume_csi_body(&mut chars);
                }
                Some(']') => {
                    chars.next();
                    consume_string_sequence_body(&mut chars, true);
                }
                Some('P') | Some('X') | Some('^') | Some('_') => {
                    chars.next();
                    consume_string_sequence_body(&mut chars, false);
                }
                // Bare ESC (or a two-byte escape): drop the introducer; any
                // remaining byte is either consumed by later rules or plain
                // printable text.
                _ => {}
            },
            // C1 forms of the same introducers.
            '\u{9b}' => consume_csi_body(&mut chars),
            '\u{9d}' => consume_string_sequence_body(&mut chars, true),
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                consume_string_sequence_body(&mut chars, false)
            }
            '\r' => {}
            _ if ch.is_control() && !matches!(ch, '\n' | '\t') => {}
            _ if is_invisible_format_char(ch) => {}
            _ => output.push(ch),
        }
    }
    output
}

/// Consumes a CSI body: everything up to and including the final byte in
/// `@..=~`. An unterminated sequence consumes to the end (fail closed).
fn consume_csi_body(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for next in chars.by_ref() {
        if ('@'..='~').contains(&next) {
            break;
        }
    }
}

/// Consumes an OSC/DCS/SOS/PM/APC payload up to its terminator.
///
/// ST (`ESC \` or C1 `U+009C`) always terminates; OSC additionally accepts
/// BEL. A bare ESC that is NOT part of ST stays inside the payload and
/// consumption continues — releasing the string early would let the rest of
/// a malformed payload flow into the output. Unterminated payloads consume
/// to the end (fail closed) so they can never leak.
fn consume_string_sequence_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    bel_terminates: bool,
) {
    while let Some(next) = chars.next() {
        match next {
            '\u{7}' if bel_terminates => break,
            '\u{9c}' => break,
            '\x1b' if chars.peek() == Some(&'\\') => {
                chars.next();
                break;
            }
            // A bare ESC that is not ST belongs to the (malformed) payload;
            // keep consuming until a real terminator or the end of input.
            _ => {}
        }
    }
}

/// Whether the character is an invisible Unicode format / default-ignorable
/// code point that is security-relevant for terminal display.
///
/// Mirrors the normative `Default_Ignorable_Code_Point` ranges from Unicode
/// `DerivedCoreProperties.txt` (including the reserved code points inside
/// them, e.g. `U+2065`, `U+FFF0..=U+FFF8`, and the unassigned parts of the
/// plane-14 tag/variation-selector block), plus the `Cf` interlinear
/// annotation controls. Dropping them may cosmetically alter rare legitimate
/// content (e.g. ZWJ emoji families) — an accepted trade-off for untrusted
/// subprocess output, where the same characters spoof what the user believes
/// they are reading or split sensitive keywords past redaction.
fn is_invisible_format_char(ch: char) -> bool {
    matches!(ch,
        '\u{00AD}' // soft hyphen
        | '\u{034F}' // combining grapheme joiner
        | '\u{061C}' // arabic letter mark
        | '\u{115F}' | '\u{1160}' // hangul fillers
        | '\u{17B4}' | '\u{17B5}'
        | '\u{180B}'..='\u{180F}' // mongolian selectors + vowel separator
        | '\u{200B}'..='\u{200F}' // zero-width space/joiners, LRM/RLM
        | '\u{202A}'..='\u{202E}' // bidi embedding/override
        | '\u{2060}'..='\u{206F}' // word joiner, invisible operators,
                                  // reserved U+2065, bidi isolates,
                                  // deprecated format
        | '\u{3164}' | '\u{FFA0}' // hangul filler compatibility forms
        | '\u{FE00}'..='\u{FE0F}' // variation selectors
        | '\u{FEFF}' // zero-width no-break space / BOM
        | '\u{FFF0}'..='\u{FFFB}' // reserved FFF0..FFF8 + interlinear
                                  // annotation
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}' // musical format controls
        | '\u{E0000}'..='\u{E0FFF}' // whole plane-14 default-ignorable
                                    // block: tags, variation selectors
                                    // supplement, and reserved gaps
    )
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
