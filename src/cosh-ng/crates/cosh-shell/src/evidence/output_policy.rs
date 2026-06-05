use std::path::Path;

use crate::evidence::model::{EvidenceExcerpt, OutputExcerptDirection};
use cosh_shell::context_window::redact_provider_command_text;
use cosh_shell::types::{CommandBlock, COMMAND_OUTPUT_REF_MAX_BYTES};

const PROVIDER_PREVIEW_MAX_CHARS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvidenceView {
    pub(crate) provider_summary: String,
    pub(crate) provider_preview: Option<String>,
    pub(crate) return_display: Option<String>,
    pub(crate) redaction_status: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EvidenceFacts<'a> {
    pub(crate) shell_session_id: &'a str,
    pub(crate) command_id: &'a str,
    pub(crate) command: &'a str,
    pub(crate) cwd: &'a str,
    pub(crate) end_cwd: &'a str,
    pub(crate) status: &'a str,
    pub(crate) exit_code: i32,
    pub(crate) duration_ms: u64,
    pub(crate) output_ref: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct TerminalOutputId {
    pub(crate) shell_session_id: String,
    pub(crate) command_id: String,
}

pub(crate) fn terminal_output_id(shell_session_id: &str, command_id: &str) -> String {
    format!("terminal-output://{shell_session_id}/{command_id}")
}

#[allow(dead_code)]
pub(crate) fn parse_terminal_output_id(output_id: &str) -> Option<TerminalOutputId> {
    let rest = output_id.strip_prefix("terminal-output://")?;
    let (shell_session_id, command_id) = rest.split_once('/')?;
    if shell_session_id.is_empty() || command_id.is_empty() || command_id.contains('/') {
        return None;
    }
    Some(TerminalOutputId {
        shell_session_id: shell_session_id.to_string(),
        command_id: command_id.to_string(),
    })
}

#[allow(dead_code)]
pub(crate) fn command_output_ref_for_id<'a>(
    blocks: &'a [CommandBlock],
    output_id: &str,
) -> Option<&'a str> {
    let parsed = parse_terminal_output_id(output_id)?;
    blocks
        .iter()
        .find(|block| block.session_id == parsed.shell_session_id && block.id == parsed.command_id)?
        .output
        .terminal_output_ref
        .as_deref()
}

#[allow(dead_code)]
pub(crate) fn bounded_output_excerpt_for_id(
    blocks: &[CommandBlock],
    output_id: &str,
    direction: OutputExcerptDirection,
    max_lines: usize,
    max_bytes: usize,
) -> EvidenceExcerpt {
    bounded_output_excerpt(
        command_output_ref_for_id(blocks, output_id),
        direction,
        max_lines,
        max_bytes,
    )
}

#[allow(dead_code)]
pub(crate) fn bounded_output_excerpt_for_command_id(
    blocks: &[CommandBlock],
    command_id: &str,
    direction: OutputExcerptDirection,
    max_lines: usize,
    max_bytes: usize,
) -> Option<EvidenceExcerpt> {
    blocks
        .iter()
        .find(|block| block.id == command_id)
        .map(|block| bounded_output_excerpt_for_block(block, direction, max_lines, max_bytes))
}

pub(crate) fn bounded_output_excerpt_for_block(
    block: &CommandBlock,
    direction: OutputExcerptDirection,
    max_lines: usize,
    max_bytes: usize,
) -> EvidenceExcerpt {
    bounded_output_excerpt(
        block.output.terminal_output_ref.as_deref(),
        direction,
        max_lines,
        max_bytes,
    )
}

pub(crate) fn output_excerpt_status_for_block(block: &CommandBlock) -> &'static str {
    if block.output.terminal_output_ref.is_none() {
        return "unavailable";
    }
    if block.output.terminal_output_bytes as usize > COMMAND_OUTPUT_REF_MAX_BYTES {
        "truncated_at_capture"
    } else {
        "available"
    }
}

#[allow(dead_code)]
pub(crate) fn bounded_output_excerpt(
    output_ref: Option<&str>,
    direction: OutputExcerptDirection,
    max_lines: usize,
    max_bytes: usize,
) -> EvidenceExcerpt {
    let Some(output_ref) = output_ref else {
        return unavailable_excerpt();
    };
    let Ok(text) = std::fs::read_to_string(Path::new(output_ref)) else {
        return unavailable_excerpt();
    };

    let text = clean_terminal_control_sequences(&text);
    let (line_bounded, line_truncated) = select_output_lines(&text, direction, max_lines.max(1));
    let (redacted, found_sensitive) = redact_sensitive_output(&line_bounded);
    let (byte_bounded, byte_truncated) = truncate_utf8_bytes(&redacted, max_bytes.max(1));
    let truncated = line_truncated || byte_truncated;
    let redaction_status = if found_sensitive {
        "excerpt_redacted"
    } else {
        "excerpt_included"
    };

    EvidenceExcerpt {
        text: Some(byte_bounded),
        status: if truncated { "truncated" } else { "included" },
        redaction_status,
        truncated,
    }
}

#[allow(dead_code)]
fn unavailable_excerpt() -> EvidenceExcerpt {
    EvidenceExcerpt {
        text: None,
        status: "unavailable",
        redaction_status: "excerpt_unavailable",
        truncated: false,
    }
}

pub(crate) fn shell_evidence_view(facts: EvidenceFacts<'_>) -> EvidenceView {
    let preview = provider_output_preview(facts.output_ref);
    let output_id = facts
        .output_ref
        .map(|_| terminal_output_id(facts.shell_session_id, facts.command_id))
        .unwrap_or_else(|| "<none>".to_string());
    let provider_preview = preview.text;
    let redaction_status = preview.redaction_status;
    let bounded_output = provider_preview.as_deref().unwrap_or(preview.reason);
    let provider_summary = format!(
        "command: {command}\n\
         cwd: {cwd}\n\
         end_cwd: {end_cwd}\n\
         status: {status}\n\
         exit_code: {exit_code}\n\
         duration_ms: {duration_ms}\n\
         output_id: {output_id}\n\
         redaction_status: {redaction_status}\n\
         bounded_output_summary:\n{bounded_output}",
        command = redact_provider_command_text(facts.command),
        cwd = facts.cwd,
        end_cwd = facts.end_cwd,
        status = facts.status,
        exit_code = facts.exit_code,
        duration_ms = facts.duration_ms,
    );

    EvidenceView {
        provider_summary,
        return_display: provider_preview.clone(),
        provider_preview,
        redaction_status,
    }
}

struct ProviderOutputPreview {
    text: Option<String>,
    redaction_status: &'static str,
    reason: &'static str,
}

fn provider_output_preview(output_ref: Option<&str>) -> ProviderOutputPreview {
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

fn redact_sensitive_output(text: &str) -> (String, bool) {
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

fn clean_terminal_control_sequences(text: &str) -> String {
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

#[allow(dead_code)]
fn select_output_lines(
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

#[allow(dead_code)]
fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }

    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}... <truncated>", &value[..end]), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_view_redacts_common_secret_shapes() {
        let dir =
            std::env::temp_dir().join(format!("cosh-shell-evidence-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("output.txt");
        std::fs::write(
            &output,
            "Authorization: Bearer abc.def.ghi\naws=AKIA1234567890ABCDEF\n-----BEGIN PRIVATE KEY-----\n",
        )
        .expect("write output");

        let view = shell_evidence_view(EvidenceFacts {
            shell_session_id: "raw-test",
            command_id: "cmd-1",
            command: "cat secret.txt",
            cwd: "/tmp",
            end_cwd: "/tmp",
            status: "completed",
            exit_code: 0,
            duration_ms: 12,
            output_ref: Some(output.to_str().expect("utf8 output path")),
        });

        assert_eq!(view.redaction_status, "preview_redacted");
        assert!(view.provider_summary.contains("command: cat secret.txt"));
        assert!(view
            .provider_summary
            .contains("output_id: terminal-output://raw-test/cmd-1"));
        assert!(!view.provider_summary.contains(output.to_str().unwrap()));
        assert!(view.provider_summary.contains("bounded_output_summary:"));
        assert!(!view.provider_summary.contains("abc.def.ghi"));
        assert!(!view.provider_summary.contains("AKIA1234567890ABCDEF"));
        assert!(!view.provider_summary.contains("BEGIN PRIVATE KEY"));
        assert!(view.provider_summary.contains("Bearer <redacted>"));
        assert!(view.provider_summary.contains("AKIA<redacted>"));
    }

    #[test]
    fn evidence_view_redacts_secret_like_command_values() {
        let view = shell_evidence_view(EvidenceFacts {
            shell_session_id: "raw-test",
            command_id: "cmd-secret",
            command: "curl https://example.test/api?password=query-secret --token cli-secret",
            cwd: "/tmp",
            end_cwd: "/tmp",
            status: "completed",
            exit_code: 0,
            duration_ms: 12,
            output_ref: None,
        });

        assert!(view.provider_summary.contains(
            "command: curl https://example.test/api?password=<redacted> --token <redacted>"
        ));
        assert!(!view.provider_summary.contains("query-secret"));
        assert!(!view.provider_summary.contains("cli-secret"));
    }

    #[test]
    fn evidence_view_truncates_long_output() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-policy-long-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("output.txt");
        std::fs::write(&output, "x".repeat(PROVIDER_PREVIEW_MAX_CHARS + 10)).expect("write output");

        let view = shell_evidence_view(EvidenceFacts {
            shell_session_id: "raw-test",
            command_id: "cmd-2",
            command: "yes",
            cwd: "/tmp",
            end_cwd: "/tmp",
            status: "completed",
            exit_code: 0,
            duration_ms: 12,
            output_ref: Some(output.to_str().expect("utf8 output path")),
        });

        let provider_preview = view.provider_preview.expect("provider preview");
        assert_eq!(view.redaction_status, "preview_redacted");
        assert!(provider_preview.ends_with("... <truncated>"));
        assert!(view
            .provider_summary
            .contains("redaction_status: preview_redacted"));
    }

    #[test]
    fn bounded_excerpt_reads_head_and_tail() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-excerpt-lines-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("output.txt");
        std::fs::write(&output, "one\ntwo\nthree\nfour\n").expect("write output");
        let output_ref = output.to_str().expect("utf8 output path");

        let head = bounded_output_excerpt(Some(output_ref), OutputExcerptDirection::Head, 2, 1024);
        assert_eq!(head.text.as_deref(), Some("one\ntwo"));
        assert_eq!(head.status, "truncated");
        assert!(head.truncated);

        let tail = bounded_output_excerpt(Some(output_ref), OutputExcerptDirection::Tail, 2, 1024);
        assert_eq!(tail.text.as_deref(), Some("three\nfour"));
        assert_eq!(tail.status, "truncated");
        assert!(tail.truncated);
    }

    #[test]
    fn bounded_excerpt_respects_line_and_byte_caps() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-excerpt-caps-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("output.txt");
        std::fs::write(&output, "alpha\nbravo\ncharlie\n").expect("write output");
        let output_ref = output.to_str().expect("utf8 output path");

        let excerpt = bounded_output_excerpt(Some(output_ref), OutputExcerptDirection::Head, 3, 8);
        assert_eq!(excerpt.text.as_deref(), Some("alpha\nbr... <truncated>"));
        assert_eq!(excerpt.status, "truncated");
        assert!(excerpt.truncated);
    }

    #[test]
    fn bounded_excerpt_preserves_utf8_boundary() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-excerpt-utf8-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("output.txt");
        std::fs::write(&output, "你好世界\n").expect("write output");
        let output_ref = output.to_str().expect("utf8 output path");

        let excerpt = bounded_output_excerpt(Some(output_ref), OutputExcerptDirection::Head, 1, 5);
        assert_eq!(excerpt.text.as_deref(), Some("你... <truncated>"));
        assert_eq!(excerpt.status, "truncated");
        assert!(excerpt.truncated);
    }

    #[test]
    fn bounded_excerpt_redacts_sensitive_output() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-excerpt-redact-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("output.txt");
        std::fs::write(&output, "Authorization: Bearer abc.def.ghi\n").expect("write output");
        let output_ref = output.to_str().expect("utf8 output path");

        let excerpt =
            bounded_output_excerpt(Some(output_ref), OutputExcerptDirection::Tail, 10, 1024);
        assert_eq!(excerpt.redaction_status, "excerpt_redacted");
        assert!(!excerpt
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("abc.def.ghi"));
        assert!(excerpt
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("Bearer <redacted>"));
    }

    #[test]
    fn bounded_excerpt_cleans_terminal_control_sequences() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-excerpt-ansi-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("output.txt");
        std::fs::write(&output, "\x1b[31mred\x1b[0m\r\nplain\x07\n").expect("write output");
        let output_ref = output.to_str().expect("utf8 output path");

        let excerpt =
            bounded_output_excerpt(Some(output_ref), OutputExcerptDirection::Head, 2, 1024);

        assert_eq!(excerpt.text.as_deref(), Some("red\nplain\n"));
    }

    #[test]
    fn bounded_excerpt_unavailable_without_ref() {
        let excerpt = bounded_output_excerpt(None, OutputExcerptDirection::Tail, 10, 1024);
        assert_eq!(excerpt.text, None);
        assert_eq!(excerpt.status, "unavailable");
        assert_eq!(excerpt.redaction_status, "excerpt_unavailable");
        assert!(!excerpt.truncated);
    }

    #[test]
    fn parses_terminal_output_id_strictly() {
        assert_eq!(
            parse_terminal_output_id("terminal-output://raw-1/cmd-2"),
            Some(TerminalOutputId {
                shell_session_id: "raw-1".to_string(),
                command_id: "cmd-2".to_string(),
            })
        );

        for invalid in [
            "terminal-output:/raw-1/cmd-2",
            "terminal-output://raw-1",
            "terminal-output:///cmd-2",
            "terminal-output://raw-1/",
            "terminal-output://raw-1/cmd-2/extra",
            "/tmp/cmd-2.txt",
        ] {
            assert!(parse_terminal_output_id(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn bounded_excerpt_for_id_resolves_session_local_command_output() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-excerpt-id-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("cmd-2.txt");
        std::fs::write(&output, "first\nsecond\nthird\n").expect("write output");
        let blocks = vec![command_block(
            "raw-1",
            "cmd-2",
            Some(output.to_str().expect("utf8 output path")),
        )];

        let excerpt = bounded_output_excerpt_for_id(
            &blocks,
            "terminal-output://raw-1/cmd-2",
            OutputExcerptDirection::Tail,
            2,
            1024,
        );

        assert_eq!(excerpt.text.as_deref(), Some("second\nthird"));
        assert_eq!(excerpt.status, "truncated");
    }

    #[test]
    fn bounded_excerpt_for_command_id_reads_head_and_tail() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-excerpt-command-id-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output = dir.join("cmd-3.txt");
        std::fs::write(&output, "one\ntwo\nthree\n").expect("write output");
        let blocks = vec![command_block(
            "raw-1",
            "cmd-3",
            Some(output.to_str().expect("utf8 output path")),
        )];

        let head = bounded_output_excerpt_for_command_id(
            &blocks,
            "cmd-3",
            OutputExcerptDirection::Head,
            2,
            1024,
        )
        .expect("head excerpt");
        let tail = bounded_output_excerpt_for_command_id(
            &blocks,
            "cmd-3",
            OutputExcerptDirection::Tail,
            2,
            1024,
        )
        .expect("tail excerpt");

        assert_eq!(head.text.as_deref(), Some("one\ntwo"));
        assert_eq!(tail.text.as_deref(), Some("two\nthree"));
        assert!(bounded_output_excerpt_for_command_id(
            &blocks,
            "cmd-missing",
            OutputExcerptDirection::Tail,
            2,
            1024,
        )
        .is_none());
    }

    #[test]
    fn bounded_excerpt_for_id_rejects_cross_session_and_missing_output() {
        let blocks = vec![
            command_block("raw-1", "cmd-1", Some("/tmp/internal-output.txt")),
            command_block("raw-2", "cmd-2", None),
        ];

        for output_id in [
            "terminal-output://raw-2/cmd-1",
            "terminal-output://raw-2/cmd-2",
            "/tmp/internal-output.txt",
        ] {
            let excerpt = bounded_output_excerpt_for_id(
                &blocks,
                output_id,
                OutputExcerptDirection::Tail,
                2,
                1024,
            );
            assert_eq!(excerpt.status, "unavailable", "{output_id}");
            assert_eq!(excerpt.text, None, "{output_id}");
        }
    }

    fn command_block(session_id: &str, id: &str, output_ref: Option<&str>) -> CommandBlock {
        CommandBlock {
            id: id.to_string(),
            session_id: session_id.to_string(),
            command: "echo hi".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 1,
            ended_at_ms: 2,
            duration_ms: 1,
            exit_code: 0,
            status: cosh_shell::types::CommandStatus::Completed,
            output: cosh_shell::types::OutputRefs {
                terminal_output_ref: output_ref.map(ToString::to_string),
                terminal_output_bytes: 0,
            },
        }
    }
}
