//! Bounded session summaries and picker-safe prompt previews.

use crate::provider::{MessageContent, MessageContentBlock};

use super::{PersistedSession, SessionHealth, SessionSummary};

pub(super) const MAX_PROMPT_PREVIEW_CHARS: usize = 160;
/// Maximum UTF-8 bytes retained from untrusted model metadata.
pub(super) const MAX_SUMMARY_MODEL_BYTES: usize = 256;
/// Maximum UTF-8 bytes retained from untrusted workspace metadata.
pub(super) const MAX_SUMMARY_WORKSPACE_BYTES: usize = 4096;

pub(super) fn summary_from_session(
    session: &PersistedSession,
    health: SessionHealth,
) -> SessionSummary {
    let first_prompt = session
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .find_map(|message| bounded_message_preview(&message.content));
    SessionSummary {
        session_id: session.session_id.clone(),
        workspace_scope: bounded_summary_text(
            &session.workspace_scope,
            MAX_SUMMARY_WORKSPACE_BYTES,
        ),
        created_at_ms: session.created_at_ms,
        updated_at_ms: session.updated_at_ms,
        model: (!session.model.is_empty())
            .then(|| bounded_summary_text(&session.model, MAX_SUMMARY_MODEL_BYTES)),
        message_count: session.messages.len(),
        first_prompt,
        schema_version: Some(session.schema_version),
        health,
    }
}

/// Truncates summary metadata without splitting a UTF-8 code point.
///
/// C0/C1 control characters are removed first so untrusted persisted
/// metadata can never inject terminal control sequences into a picker.
pub(crate) fn bounded_summary_text(value: &str, max_bytes: usize) -> String {
    let sanitized: String = value.chars().filter(|ch| !ch.is_control()).collect();
    if sanitized.len() <= max_bytes {
        return sanitized;
    }
    const ELLIPSIS: &str = "…";
    if max_bytes < ELLIPSIS.len() {
        return String::new();
    }
    let mut boundary = max_bytes - ELLIPSIS.len();
    while !sanitized.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}{}", &sanitized[..boundary], ELLIPSIS)
}

fn bounded_message_preview(content: &MessageContent) -> Option<String> {
    let mut preview = String::new();
    let mut char_count = 0;
    match content {
        MessageContent::Text(text) => {
            append_preview_fragment(&mut preview, &mut char_count, text);
        }
        MessageContent::Blocks(blocks) => {
            for block in blocks {
                let text = match block {
                    MessageContentBlock::Text { text }
                    | MessageContentBlock::ToolResult { content: text, .. } => text,
                };
                if append_preview_fragment(&mut preview, &mut char_count, text) {
                    break;
                }
            }
        }
    }
    if preview.is_empty() {
        return None;
    }
    if char_count > MAX_PROMPT_PREVIEW_CHARS {
        preview.pop();
        preview.pop();
        preview.push('…');
    }
    Some(preview)
}

fn append_preview_fragment(preview: &mut String, char_count: &mut usize, fragment: &str) -> bool {
    for word in fragment.split_whitespace() {
        if !preview.is_empty() {
            preview.push(' ');
            *char_count += 1;
            if *char_count > MAX_PROMPT_PREVIEW_CHARS {
                return true;
            }
        }
        // Control characters are dropped so persisted prompts cannot smuggle
        // terminal control bytes (BEL, BS, C1 CSI) into picker rows.
        for character in word.chars().filter(|character| !character.is_control()) {
            preview.push(character);
            *char_count += 1;
            if *char_count > MAX_PROMPT_PREVIEW_CHARS {
                return true;
            }
        }
    }
    false
}
