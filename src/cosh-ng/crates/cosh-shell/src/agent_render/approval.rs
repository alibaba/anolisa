use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::ROUNDED,
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, Paragraph, Widget, Wrap},
};

use super::actions::{ApprovalPanelAction, APPROVAL_PANEL_ACTIONS};

use super::{
    buffer_to_lines, buffer_to_styled_lines, char_width, display_width, RatatuiInlineRenderer,
};

#[derive(Debug, Clone)]
pub struct ApprovalPanelModel<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub risk: &'a str,
    pub reason: Option<&'a str>,
    pub subject: &'a str,
    pub preview_label: &'a str,
    pub preview: &'a str,
    pub queue_position: usize,
    pub queue_total: usize,
    pub next_label: Option<&'a str>,
    pub selected_action: ApprovalPanelAction,
    pub expanded: bool,
}

impl RatatuiInlineRenderer {
    pub fn write_approval_panel<W: Write>(
        &self,
        output: &mut W,
        model: ApprovalPanelModel<'_>,
    ) -> io::Result<usize> {
        let lines = self.approval_panel_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn approval_panel_lines(&self, model: ApprovalPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_approval_panel_lines(model);
        }

        let width = self.panel_standard_width();
        let height = approval_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_panel(model, self.i18n(), area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn approval_panel_write_lines(&self, model: ApprovalPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_approval_panel_lines(model);
        }

        let width = self.panel_standard_width();
        let height = approval_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_panel(model, self.i18n(), area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_approval_panel_lines(&self, model: ApprovalPanelModel<'_>) -> Vec<String> {
        let i18n = self.i18n();
        if is_command_approval_request(&model) {
            let command_rows = command_preview_rows(
                model.preview,
                self.content_width(),
                max_preview_rows(model.expanded),
            );
            let mut lines = vec![
                i18n.t(crate::MessageId::ApprovalRequiredTitle).to_string(),
                command_request_heading(model.subject, i18n).to_string(),
            ];
            if let Some(reason) = model.reason {
                lines.push(approval_reason_line(reason, i18n));
            }
            lines.extend(command_rows);
            if model.queue_total > 1 {
                let position = model.queue_position.to_string();
                let total = model.queue_total.to_string();
                let mut queue = i18n.format(
                    crate::MessageId::ApprovalQueueCompactLine,
                    &[("position", position.as_str()), ("total", total.as_str())],
                );
                if let Some(next) = model.next_label {
                    queue.push_str(
                        &i18n.format(crate::MessageId::ApprovalQueueNextSuffix, &[("next", next)]),
                    );
                }
                lines.push(queue);
            }
            lines.push(approval_action_line(model.selected_action, i18n));
            if model.expanded {
                lines.push(
                    i18n.t(crate::MessageId::ApprovalCommandDefaultPolicy)
                        .to_string(),
                );
                lines.push(format!(
                    "{}{}",
                    i18n.t(crate::MessageId::ApprovalKeysPrefix),
                    i18n.t(crate::MessageId::ApprovalKeysText)
                ));
            }
            return lines;
        }

        let position = model.queue_position.to_string();
        let total = model.queue_total.to_string();
        let risk = i18n.format(
            crate::MessageId::ApprovalRiskSuffix,
            &[("risk", model.risk)],
        );
        let mut lines = vec![
            i18n.t(crate::MessageId::ApprovalRequiredTitle).to_string(),
            format!("{} · {} · {}", model.id, model.kind, risk),
            i18n.format(
                crate::MessageId::ApprovalQueueFullLine,
                &[("position", position.as_str()), ("total", total.as_str())],
            ),
            format!(
                "{}{}",
                i18n.t(crate::MessageId::ApprovalSubjectLabel),
                model.subject
            ),
            format!("{}: {}", model.preview_label, model.preview),
        ];
        if let Some(reason) = model.reason {
            lines.push(approval_reason_line(reason, i18n));
        }
        if let Some(next) = model.next_label {
            lines.push(format!(
                "{}{next}",
                i18n.t(crate::MessageId::ApprovalNextLabel)
            ));
        }
        lines.push(approval_action_line(model.selected_action, i18n));
        if model.expanded {
            lines.push(
                i18n.t(crate::MessageId::ApprovalExecutableToolPolicy)
                    .to_string(),
            );
            lines.push(format!(
                "{}{}",
                i18n.t(crate::MessageId::ApprovalKeysPrefix),
                i18n.t(crate::MessageId::ApprovalKeysText)
            ));
        }
        lines
    }
}

fn approval_panel_height(model: &ApprovalPanelModel<'_>, width: u16) -> u16 {
    let content_width = approval_content_width(width);
    if is_command_approval_request(model) {
        let command_rows = command_preview_rows(
            model.preview,
            content_width,
            max_preview_rows(model.expanded),
        )
        .len()
        .max(1) as u16;
        let queue_rows = u16::from(model.queue_total > 1);
        let reason_rows = model
            .reason
            .map(|reason| {
                approval_reason_rows(
                    reason,
                    content_width,
                    crate::I18n::new(crate::Language::EnUs),
                )
                .len() as u16
            })
            .unwrap_or(0);
        let expanded_rows = if model.expanded { 2 } else { 0 };
        return 4 + command_rows + queue_rows + reason_rows + expanded_rows;
    }

    let preview_rows = wrapped_preview_rows(
        model.preview,
        content_width,
        max_preview_rows(model.expanded),
    )
    .len()
    .max(1) as u16;
    let next_rows = u16::from(model.next_label.is_some());
    let reason_rows = model
        .reason
        .map(|reason| {
            approval_reason_rows(
                reason,
                content_width,
                crate::I18n::new(crate::Language::EnUs),
            )
            .len() as u16
        })
        .unwrap_or(0);
    let policy_rows = if model.expanded { 2 } else { 0 };
    7 + preview_rows + next_rows + reason_rows + policy_rows
}

fn render_approval_panel(
    model: ApprovalPanelModel<'_>,
    i18n: crate::I18n,
    area: Rect,
    buffer: &mut Buffer,
) {
    if is_command_approval_request(&model) {
        render_command_tool_approval_panel(model, i18n, area, buffer);
        return;
    }

    let border = if model.risk == "high" {
        Color::Red
    } else {
        Color::Yellow
    };
    let block = Block::bordered()
        .padding(Padding::horizontal(1))
        .title(Line::from(vec![
            Span::styled(
                format!(" {} ", i18n.t(crate::MessageId::ApprovalTitle)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} ", model.id)),
        ]))
        .border_set(ROUNDED)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    block.render(area, buffer);

    let preview_rows = wrapped_preview_rows(
        model.preview,
        inner.width.saturating_sub(2) as usize,
        max_preview_rows(model.expanded),
    );
    let preview_height = preview_rows.len().max(1) as u16;
    let next_height = u16::from(model.next_label.is_some());
    let reason_rows = model
        .reason
        .map(|reason| approval_reason_rows(reason, inner.width.saturating_sub(2) as usize, i18n))
        .unwrap_or_default();
    let reason_height = reason_rows.len() as u16;
    let mut constraints = vec![
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(preview_height),
        Constraint::Length(next_height),
        Constraint::Length(reason_height),
        Constraint::Length(1),
        Constraint::Length(1),
    ];
    if model.expanded {
        constraints.push(Constraint::Length(2));
    }
    let chunks = Layout::vertical(constraints).split(inner);

    Paragraph::new(Line::from(vec![
        Span::styled(model.kind, Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled(
            i18n.format(
                crate::MessageId::ApprovalRiskSuffix,
                &[("risk", model.risk)],
            ),
            Style::default().fg(border),
        ),
        Span::raw(format!(
            "  {}",
            i18n.format(
                crate::MessageId::ApprovalQueueCompactLine,
                &[
                    ("position", model.queue_position.to_string().as_str()),
                    ("total", model.queue_total.to_string().as_str()),
                ],
            )
        )),
    ]))
    .render(chunks[0], buffer);

    Paragraph::new(Line::from(vec![
        Span::styled(
            i18n.t(crate::MessageId::ApprovalSubjectLabel),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(model.subject),
    ]))
    .render(chunks[1], buffer);

    Paragraph::new(Line::from(Span::styled(
        format!("{}:", model.preview_label),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )))
    .render(chunks[2], buffer);

    let preview_lines = preview_rows
        .into_iter()
        .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::White))))
        .collect::<Vec<_>>();
    Paragraph::new(Text::from(preview_lines)).render(chunks[3], buffer);

    if let Some(next) = model.next_label {
        Paragraph::new(Line::from(vec![
            Span::styled(
                i18n.t(crate::MessageId::ApprovalNextLabel),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(next.to_string()),
        ]))
        .render(chunks[4], buffer);
    }

    if !reason_rows.is_empty() {
        Paragraph::new(Text::from(
            reason_rows
                .into_iter()
                .map(|line| Line::from(Span::raw(line)))
                .collect::<Vec<_>>(),
        ))
        .render(chunks[5], buffer);
    }

    Paragraph::new(approval_action_spans(model.selected_action, i18n)).render(chunks[6], buffer);

    Paragraph::new(Line::from(vec![
        Span::styled(
            i18n.t(crate::MessageId::ApprovalKeysPrefix),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(i18n.t(crate::MessageId::ApprovalKeysText)),
    ]))
    .render(chunks[7], buffer);

    if model.expanded {
        Paragraph::new(Text::from(vec![
            Line::from(i18n.t(crate::MessageId::ApprovalExecutableToolPolicy)),
            Line::from(i18n.t(crate::MessageId::ApprovalExecutableToolPolicyExtra)),
        ]))
        .wrap(Wrap { trim: true })
        .render(chunks[8], buffer);
    }
}

fn render_command_tool_approval_panel(
    model: ApprovalPanelModel<'_>,
    i18n: crate::I18n,
    area: Rect,
    buffer: &mut Buffer,
) {
    let border = if model.risk == "high" {
        Color::Red
    } else {
        Color::Yellow
    };
    let block = Block::bordered()
        .padding(Padding::horizontal(1))
        .title(Line::from(vec![
            Span::styled(
                format!(" {} ", i18n.t(crate::MessageId::ApprovalTitle)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} ", model.id)),
        ]))
        .border_set(ROUNDED)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    block.render(area, buffer);
    let command_rows = command_preview_rows(
        model.preview,
        inner.width.saturating_sub(2) as usize,
        max_preview_rows(model.expanded),
    );
    let reason_rows = model
        .reason
        .map(|reason| approval_reason_rows(reason, inner.width.saturating_sub(2) as usize, i18n))
        .unwrap_or_default();
    let queue_height = u16::from(model.queue_total > 1);
    let mut constraints = vec![
        Constraint::Length(1),
        Constraint::Length(reason_rows.len() as u16),
        Constraint::Length(command_rows.len().max(1) as u16),
        Constraint::Length(queue_height),
        Constraint::Length(1),
    ];
    if model.expanded {
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Length(1));
    }
    let chunks = Layout::vertical(constraints).split(inner);
    let action_index = 4;
    let keys_index = 5;
    let policy_index = 6;

    Paragraph::new(Line::from(Span::styled(
        command_request_heading(model.subject, i18n),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )))
    .render(chunks[0], buffer);

    if !reason_rows.is_empty() {
        Paragraph::new(Text::from(
            reason_rows
                .into_iter()
                .map(|line| Line::from(Span::raw(line)))
                .collect::<Vec<_>>(),
        ))
        .render(chunks[1], buffer);
    }

    let command_lines = command_rows
        .into_iter()
        .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::White))))
        .collect::<Vec<_>>();
    Paragraph::new(Text::from(command_lines)).render(chunks[2], buffer);

    if model.queue_total > 1 {
        let position = model.queue_position.to_string();
        let total = model.queue_total.to_string();
        let mut queue = i18n.format(
            crate::MessageId::ApprovalQueueCompactLine,
            &[("position", position.as_str()), ("total", total.as_str())],
        );
        if let Some(next) = model.next_label {
            queue.push_str(
                &i18n.format(crate::MessageId::ApprovalQueueNextSuffix, &[("next", next)]),
            );
        }
        Paragraph::new(Line::from(Span::styled(
            queue,
            Style::default().fg(Color::DarkGray),
        )))
        .render(chunks[3], buffer);
    }

    Paragraph::new(approval_action_spans(model.selected_action, i18n))
        .render(chunks[action_index], buffer);

    if model.expanded {
        Paragraph::new(Line::from(vec![
            Span::styled(
                i18n.t(crate::MessageId::ApprovalKeysPrefix),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(i18n.t(crate::MessageId::ApprovalKeysText)),
        ]))
        .render(chunks[keys_index], buffer);
        Paragraph::new(i18n.t(crate::MessageId::ApprovalCommandDefaultPolicy))
            .wrap(Wrap { trim: true })
            .render(chunks[policy_index], buffer);
    }
}

fn approval_action_spans(selected: ApprovalPanelAction, i18n: crate::I18n) -> Line<'static> {
    let mut spans = Vec::new();
    for (idx, descriptor) in APPROVAL_PANEL_ACTIONS.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(action_span(
            approval_action_label(descriptor.action, i18n),
            descriptor.action,
            selected == descriptor.action,
        ));
    }
    Line::from(spans)
}

fn approval_action_line(selected: ApprovalPanelAction, i18n: crate::I18n) -> String {
    APPROVAL_PANEL_ACTIONS
        .iter()
        .map(|descriptor| {
            let label = approval_action_label(descriptor.action, i18n);
            if descriptor.action == selected {
                format!("[{label}]")
            } else {
                label.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn approval_reason_line(reason: &str, i18n: crate::I18n) -> String {
    i18n.format(
        crate::MessageId::ApprovalAssessmentReasonLine,
        &[("reason", reason)],
    )
}

fn approval_reason_rows(reason: &str, width: usize, i18n: crate::I18n) -> Vec<String> {
    wrapped_preview_rows(&approval_reason_line(reason, i18n), width, 2)
}

fn approval_action_label(action: ApprovalPanelAction, i18n: crate::I18n) -> &'static str {
    match action {
        ApprovalPanelAction::Approve => i18n.t(crate::MessageId::ApprovalActionAllowOnce),
        ApprovalPanelAction::AlwaysTrust => i18n.t(crate::MessageId::ApprovalActionAlwaysTrust),
        ApprovalPanelAction::Deny => i18n.t(crate::MessageId::ApprovalActionDeny),
        ApprovalPanelAction::Details => i18n.t(crate::MessageId::ApprovalActionDetails),
    }
}

fn action_span(label: &str, action: ApprovalPanelAction, selected: bool) -> Span<'static> {
    if selected {
        Span::styled(format!("> [ {label} ] "), selected_action_style(action))
    } else {
        Span::styled(format!("  [ {label} ] "), Style::default().fg(Color::Gray))
    }
}

fn selected_action_style(action: ApprovalPanelAction) -> Style {
    let background = match action {
        ApprovalPanelAction::Approve => Color::Green,
        ApprovalPanelAction::AlwaysTrust => Color::Cyan,
        ApprovalPanelAction::Deny => Color::Red,
        ApprovalPanelAction::Details => Color::Blue,
    };
    Style::default()
        .fg(Color::White)
        .bg(background)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn approval_content_width(width: u16) -> usize {
    width.saturating_sub(4).max(20) as usize
}

fn max_preview_rows(expanded: bool) -> usize {
    if expanded {
        6
    } else {
        3
    }
}

fn is_command_approval_request(model: &ApprovalPanelModel<'_>) -> bool {
    (model.kind == "tool request"
        && (model.subject.eq_ignore_ascii_case("tool Bash")
            || model.subject.eq_ignore_ascii_case("tool shell")))
        || (model.kind == "shell command request"
            && model.subject.eq_ignore_ascii_case("shell command"))
}

fn command_request_heading(subject: &str, i18n: crate::I18n) -> &'static str {
    if subject.eq_ignore_ascii_case("tool shell") || subject.eq_ignore_ascii_case("shell command") {
        i18n.t(crate::MessageId::ApprovalRunShellCommandPrompt)
    } else {
        i18n.t(crate::MessageId::ApprovalRunBashCommandPrompt)
    }
}

fn command_preview_rows(command: &str, width: usize, max_rows: usize) -> Vec<String> {
    let rows = wrapped_preview_rows(command, width.saturating_sub(2).max(20), max_rows);
    if rows.is_empty() {
        return vec!["$".to_string()];
    }
    rows.into_iter()
        .enumerate()
        .map(|(idx, row)| {
            if idx == 0 {
                format!("$ {row}")
            } else {
                format!("  {row}")
            }
        })
        .collect()
}

pub(super) fn wrapped_preview_rows(text: &str, width: usize, max_rows: usize) -> Vec<String> {
    let width = width.max(20);
    let mut rows = Vec::new();
    for raw_line in text.lines() {
        let mut current = String::new();
        let mut current_width = 0;
        for ch in raw_line.chars() {
            let ch_width = char_width(ch);
            if current_width + ch_width > width && !current.is_empty() {
                rows.push(current);
                if rows.len() == max_rows {
                    return ellipsize_last_row(rows, width);
                }
                current = String::new();
                current_width = 0;
            }
            current.push(ch);
            current_width += ch_width;
        }
        if !current.is_empty() || raw_line.is_empty() {
            rows.push(current);
            if rows.len() == max_rows {
                return ellipsize_last_row(rows, width);
            }
        }
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn ellipsize_last_row(mut rows: Vec<String>, width: usize) -> Vec<String> {
    if let Some(last) = rows.last_mut() {
        while display_width(last) + 4 > width {
            if last.pop().is_none() {
                break;
            }
        }
        last.push_str(" ...");
    }
    rows
}
