use std::time::{SystemTime, UNIX_EPOCH};

use crate::adapter::{SessionErrorInfo, SessionSummary};
use crate::runtime::prelude::*;
use crate::slash::panel::render_notice_panel;

use super::RuntimeSessionPanelPhase;

const SESSION_VIEWPORT_SIZE: usize = 8;
const SESSION_PREVIEW_CHARS: usize = 72;

pub(super) fn render_current_session_panel<W: Write>(
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let Some(panel) = state.control.session().pending_panel() else {
        return Ok(());
    };
    if state.control.session().active_panel_id() == Some(panel.id.as_str()) {
        return Ok(());
    }
    let (title, body, footer) =
        match panel.phase {
            RuntimeSessionPanelPhase::Browse => {
                let active = core_adapter(adapter).and_then(|core| core.committed_session_id());
                let selected = core_adapter(adapter)
                    .and_then(|core| core.recovery_snapshot().selected_session_id);
                let (start, end) = session_viewport(
                    panel.sessions.len(),
                    panel.selected_option,
                    SESSION_VIEWPORT_SIZE,
                );
                let mut body = Vec::new();
                if start > 0 {
                    body.push(format!("  … +{start}"));
                }
                body.extend(panel.sessions[start..end].iter().enumerate().map(
                    |(offset, summary)| {
                        let index = start + offset;
                        session_summary_line(
                            summary,
                            panel.selected_option == index,
                            panel.selected_for_clear.contains(&summary.session_id),
                            active.as_deref() == Some(summary.session_id.as_str())
                                || selected.as_deref() == Some(summary.session_id.as_str()),
                        )
                    },
                ));
                if end < panel.sessions.len() {
                    body.push(format!("  … +{}", panel.sessions.len() - end));
                }
                let footer = format!(
                    "{} · {}/{}",
                    state.i18n().t(MessageId::SessionPickerFooter),
                    panel.selected_option.saturating_add(1),
                    panel.sessions.len()
                );
                (
                    state.i18n().t(MessageId::SessionTitle).to_string(),
                    body,
                    footer,
                )
            }
            RuntimeSessionPanelPhase::ConfirmClear => {
                let count = panel.clear_confirmation_ids.len().to_string();
                let mut body = vec![state.i18n().format(
                    MessageId::SessionClearConfirmCountLine,
                    &[("count", &count)],
                )];
                body.extend(
                    panel
                        .clear_confirmation_ids
                        .iter()
                        .take(SESSION_VIEWPORT_SIZE)
                        .map(|session_id| format!("  {session_id}")),
                );
                if panel.clear_confirmation_ids.len() > SESSION_VIEWPORT_SIZE {
                    body.push(format!(
                        "  … +{}",
                        panel.clear_confirmation_ids.len() - SESSION_VIEWPORT_SIZE
                    ));
                }
                if !panel.protected_clear_ids.is_empty() {
                    body.push(state.i18n().t(MessageId::SessionProtectedBody).to_string());
                    body.extend(
                        panel
                            .protected_clear_ids
                            .iter()
                            .take(SESSION_VIEWPORT_SIZE)
                            .map(|session_id| format!("  protected: {session_id}")),
                    );
                    if panel.protected_clear_ids.len() > SESSION_VIEWPORT_SIZE {
                        body.push(format!(
                            "  … +{} protected",
                            panel.protected_clear_ids.len() - SESSION_VIEWPORT_SIZE
                        ));
                    }
                }
                (
                    state
                        .i18n()
                        .t(MessageId::SessionClearConfirmTitle)
                        .to_string(),
                    body,
                    state
                        .i18n()
                        .t(MessageId::SessionClearConfirmFooter)
                        .to_string(),
                )
            }
        };
    let panel_id = panel.id.clone();
    render_notice_panel(output, &title, body.clone(), Some(&footer))?;
    state
        .control
        .session_mut()
        .set_active_panel(panel_id, session_notice_height(&body, Some(&footer)));
    Ok(())
}

pub(super) fn redraw_session_panel<W: Write>(
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_session_panel(state, output)?;
    render_current_session_panel(adapter, state, output)
}

pub(super) fn close_session_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_session_panel(state, output)?;
    state.control.session_mut().clear_pending_panel();
    Ok(())
}

fn clear_active_session_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let height = state.control.session().active_panel_height();
    if height == 0 {
        state.control.session_mut().clear_active_panel_id();
        return Ok(());
    }
    write!(output, "\x1b[{height}A")?;
    for row in 0..height {
        write!(output, "\r\x1b[2K")?;
        if row + 1 < height {
            write!(output, "\x1b[1B")?;
        }
    }
    if height > 1 {
        write!(output, "\x1b[{}A", height - 1)?;
    }
    write!(output, "\r")?;
    state.control.session_mut().clear_active_panel();
    Ok(())
}

fn session_notice_height(body: &[String], footer: Option<&str>) -> usize {
    let renderer = RatatuiInlineRenderer::for_terminal();
    let mut lines = body
        .iter()
        .flat_map(|line| renderer.markdown_text_lines(line))
        .collect::<Vec<_>>();
    if let Some(footer) = footer {
        lines.extend(renderer.markdown_text_lines(footer));
    }
    lines.len().max(1) + 2
}

pub(super) fn session_summary_line(
    summary: &SessionSummary,
    focused: bool,
    marked_for_clear: bool,
    protected: bool,
) -> String {
    let cursor = if focused { ">" } else { " " };
    let marked = if marked_for_clear { "[x]" } else { "[ ]" };
    let protected = if protected { " protected" } else { "" };
    let prompt = summary
        .first_prompt
        .as_deref()
        .filter(|prompt| !prompt.trim().is_empty())
        .unwrap_or(&summary.session_id);
    let prompt = bounded_prompt_preview(prompt, SESSION_PREVIEW_CHARS);
    let model = summary.model.as_deref().unwrap_or("-");
    format!(
        "{cursor} {marked} {prompt} · {} · {} msg · {} · {}{protected}",
        relative_time(summary.updated_at_ms),
        summary.message_count,
        model,
        summary.health.label()
    )
}

fn session_viewport(total: usize, selected: usize, capacity: usize) -> (usize, usize) {
    if total == 0 || capacity == 0 {
        return (0, 0);
    }
    let capacity = capacity.min(total);
    let selected = selected.min(total - 1);
    let start = selected
        .saturating_sub(capacity / 2)
        .min(total.saturating_sub(capacity));
    (start, start + capacity)
}

fn bounded_prompt_preview(prompt: &str, max_chars: usize) -> String {
    // Strip control characters so persisted metadata cannot inject terminal
    // control bytes (BEL, BS, C1 CSI) into rendered picker rows.
    let compact = prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let keep = max_chars.saturating_sub(1);
    let mut bounded = compact.chars().take(keep).collect::<String>();
    bounded.push('…');
    bounded
}

fn relative_time(updated_at_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(updated_at_ms);
    let seconds = now.saturating_sub(updated_at_ms) / 1_000;
    match seconds {
        0..=59 => "now".to_string(),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

pub(super) fn render_not_ready<W: Write>(
    summary: &SessionSummary,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    render_notice_panel(
        output,
        state.i18n().t(MessageId::SessionErrorTitle),
        vec![state.i18n().format(
            MessageId::SessionNotReadyBody,
            &[
                ("id", &summary.session_id),
                ("health", summary.health.label()),
            ],
        )],
        None,
    )
}

pub(super) fn render_session_error<W: Write>(
    state: &InlineState,
    output: &mut W,
    error: &SessionErrorInfo,
) -> std::io::Result<()> {
    let mut body = vec![state.i18n().format(
        MessageId::SessionErrorLine,
        &[("code", &error.code), ("error", &error.message)],
    )];
    if let Some(hint) = error.hint.as_deref() {
        body.push(hint.to_string());
    }
    render_notice_panel(
        output,
        state.i18n().t(MessageId::SessionErrorTitle),
        body,
        None,
    )
}

pub(super) fn render_unavailable<W: Write>(
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    render_notice_panel(
        output,
        state.i18n().t(MessageId::SessionErrorTitle),
        vec![state
            .i18n()
            .t(MessageId::SessionUnavailableBody)
            .to_string()],
        None,
    )
}

pub(super) fn render_usage<W: Write>(state: &InlineState, output: &mut W) -> std::io::Result<()> {
    render_notice_panel(
        output,
        state.i18n().t(MessageId::SessionTitle),
        vec![state.i18n().t(MessageId::SessionUsageBody).to_string()],
        None,
    )
}

/// Gate for session mutations (`clear`, `resume`, `delete`, starting a new
/// compaction): idle only when no interaction is pending *and* no part of the
/// compaction lifecycle — running compactor, finished-but-unrendered
/// completion, or recommended automatic attempt — could be invalidated by the
/// mutation.
pub(super) fn session_management_idle(state: &InlineState) -> bool {
    session_interaction_idle(state) && !crate::slash::session::compaction_pending_or_active(state)
}

/// Interaction-only idle check shared by [`session_management_idle`] and the
/// automatic compaction starter.
///
/// Deliberately excludes compaction state: the auto-start path must not be
/// blocked by its *own* pending recommendation, so it layers its own
/// active/pending-completion checks on top of this.
pub(super) fn session_interaction_idle(state: &InlineState) -> bool {
    state.agent_run.active.is_none()
        && !state
            .approvals
            .requests
            .iter()
            .any(|request| request.status == ApprovalRequestStatus::Pending)
        && state.questions.pending_id.is_none()
        && state.auth.state.is_none()
        && state.control.pending_mode_panel().is_none()
        && state.control.pending_config_panel().is_none()
        && state.control.pending_config_language_panel().is_none()
        && state.control.session().pending_panel().is_none()
}

pub(super) fn workspace_scope(blocks: &[CommandBlock]) -> String {
    let candidate = blocks.last().map(|block| {
        if block.end_cwd.is_empty() {
            block.cwd.clone()
        } else {
            block.end_cwd.clone()
        }
    });
    let path = candidate
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn core_adapter(adapter: &AdapterInstance) -> Option<&crate::adapter::CoshCoreAdapter> {
    match adapter {
        AdapterInstance::CoshCore(core) => Some(core),
        _ => None,
    }
}

pub(super) fn partition_protected(
    requested: Vec<String>,
    protected: &[String],
) -> (Vec<String>, Vec<String>) {
    requested
        .into_iter()
        .partition(|session_id| !protected.contains(session_id))
}

pub(super) enum SessionCardAction {
    Focus { id: String, selected: usize },
    Toggle { id: String, selected: usize },
    Resume { id: String, selected: usize },
    Delete { id: String },
    ConfirmClear { id: String },
    Cancel { id: String },
}

pub(super) fn session_card_action_from_event(event: &ShellEvent) -> Option<SessionCardAction> {
    if event.kind != ShellEventKind::UserInputIntercepted
        || event.component.as_deref() != Some("card")
    {
        return None;
    }
    match event.message.as_deref()? {
        "session_focus" => {
            let (id, selected) = split_session_value(event.input.as_deref()?)?;
            Some(SessionCardAction::Focus { id, selected })
        }
        "session_toggle" => {
            let (id, selected) = split_session_value(event.input.as_deref()?)?;
            Some(SessionCardAction::Toggle { id, selected })
        }
        "session_resume" => {
            let (id, selected) = split_session_value(event.input.as_deref()?)?;
            Some(SessionCardAction::Resume { id, selected })
        }
        "session_delete" => Some(SessionCardAction::Delete {
            id: event.input.as_deref()?.to_string(),
        }),
        "session_clear_confirm" => Some(SessionCardAction::ConfirmClear {
            id: event.input.as_deref()?.to_string(),
        }),
        "session_cancel" => Some(SessionCardAction::Cancel {
            id: event.input.as_deref()?.to_string(),
        }),
        _ => None,
    }
}

fn split_session_value(value: &str) -> Option<(String, usize)> {
    let (id, selected) = value.rsplit_once(':')?;
    Some((id.to_string(), selected.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::SessionHealth;

    #[test]
    fn session_lines_surface_health_and_clear_mark() {
        let summary = SessionSummary {
            session_id: "00000000-0000-4000-8000-000000000000".to_string(),
            workspace_scope: "/tmp".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            model: Some("mock".to_string()),
            message_count: 3,
            first_prompt: Some("remember this".to_string()),
            schema_version: Some(1),
            health: SessionHealth::Corrupt,
        };
        let line = session_summary_line(&summary, true, true, false);
        assert!(line.contains("> [x] remember this"));
        assert!(line.contains("3 msg"));
        assert!(line.contains("corrupt"));
    }

    #[test]
    fn session_viewport_tracks_selection_without_rendering_every_entry() {
        assert_eq!(session_viewport(30, 0, 8), (0, 8));
        assert_eq!(session_viewport(30, 15, 8), (11, 19));
        assert_eq!(session_viewport(30, 29, 8), (22, 30));
        assert_eq!(session_viewport(3, 2, 8), (0, 3));
    }

    #[test]
    fn session_preview_is_single_line_utf8_safe_and_bounded() {
        let preview = bounded_prompt_preview(
            "第一行\n第二行\twith a deliberately long suffix that must not fill the terminal",
            24,
        );
        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\t'));
        assert!(preview.chars().count() <= 24);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn session_preview_strips_terminal_control_bytes() {
        let preview = bounded_prompt_preview("safe\u{7}\u{8}\u{9b}31mtext", 24);
        assert_eq!(preview, "safe31mtext");
    }

    #[test]
    fn clear_partition_protects_active_and_selected_ids() {
        let (clearable, protected) = partition_protected(
            vec![
                "old".to_string(),
                "active".to_string(),
                "selected".to_string(),
            ],
            &["active".to_string(), "selected".to_string()],
        );
        assert_eq!(clearable, vec!["old"]);
        assert_eq!(protected, vec!["active", "selected"]);
    }

    #[test]
    fn session_management_waits_for_pending_questions() {
        let mut state = InlineState::default();
        assert!(session_management_idle(&state));

        state.questions.pending_id = Some("question-1".to_string());
        assert!(!session_management_idle(&state));
    }
}
