use crate::runtime::prelude::*;

pub(crate) fn render_approval_requests<W: Write>(
    state: &mut InlineState,
    approval_ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    if approval_ids.is_empty() {
        return Ok(());
    }

    render_current_approval_request(state, output)
}

pub(crate) fn render_current_approval_request<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let Some((index, request)) = state
        .approvals
        .requests
        .iter()
        .enumerate()
        .find(|(_, request)| request.status == ApprovalRequestStatus::Pending)
    else {
        return Ok(());
    };

    if state.approvals.active_panel_id.as_deref() == Some(request.id.as_str()) {
        return Ok(());
    }

    let pending_total = state
        .approvals
        .requests
        .iter()
        .filter(|request| request.status == ApprovalRequestStatus::Pending)
        .count();
    let pending_position = state
        .approvals
        .requests
        .iter()
        .take(index + 1)
        .filter(|request| request.status == ApprovalRequestStatus::Pending)
        .count();
    let next_pending = state
        .approvals
        .requests
        .iter()
        .skip(index + 1)
        .find(|request| request.status == ApprovalRequestStatus::Pending);
    let i18n = state.i18n();
    let preview_label = match request.kind {
        ApprovalRequestKind::Tool => i18n.t(MessageId::ApprovalToolInputLabel),
        ApprovalRequestKind::ShellCommand => i18n.t(MessageId::ApprovalCommandLabel),
    };
    let next_label = next_pending.map(|next| format!("{} {}", next.id, next.subject));
    let selected_action = state
        .approvals
        .focus
        .get(&request.id)
        .copied()
        .unwrap_or(ApprovalPanelAction::Approve);
    let expanded = state.approvals.expanded_cards.contains(&request.id);
    let height = RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_approval_panel(
            output,
            ApprovalPanelModel {
                id: &request.id,
                kind: request.kind.label(),
                risk: request.risk,
                reason: request
                    .assessment
                    .as_ref()
                    .map(|assessment| assessment.primary_reason),
                subject: &request.subject,
                preview_label,
                preview: &request.preview,
                queue_position: pending_position,
                queue_total: pending_total,
                next_label: next_label.as_deref(),
                selected_action,
                expanded,
            },
        )?;
    state.approvals.active_panel_id = Some(request.id.clone());
    state.approvals.active_panel_height = height;
    Ok(())
}

pub(crate) fn redraw_current_approval_request<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_approval_panel(state, output)?;
    render_current_approval_request(state, output)
}

pub(crate) fn clear_active_approval_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let height = state.approvals.active_panel_height;
    if height == 0 {
        state.approvals.active_panel_id = None;
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
    state.approvals.active_panel_id = None;
    state.approvals.active_panel_height = 0;
    Ok(())
}

pub(crate) fn approval_is_pending(state: &InlineState, id: &str) -> bool {
    state
        .approvals
        .requests
        .iter()
        .any(|request| request.id == id && request.status == ApprovalRequestStatus::Pending)
}

pub(crate) fn approval_focus_from_event(
    event: &ShellEvent,
) -> Option<(String, ApprovalPanelAction)> {
    if event.kind != ShellEventKind::UserInputIntercepted
        || event.component.as_deref() != Some("card")
        || event.message.as_deref() != Some("focus")
    {
        return None;
    }

    let (id, selected) = event.input.as_deref()?.split_once(':')?;
    let index = selected.trim().parse::<usize>().ok()?;
    let action = approval_action_at(index)?;
    Some((id.trim().to_string(), action))
}
