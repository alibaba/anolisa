use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, Paragraph, Widget, Wrap},
};

use crate::approval_actions::{ApprovalPanelAction, APPROVAL_PANEL_ACTIONS};

use super::{
    buffer_to_lines, buffer_to_styled_lines, char_width, display_width, RatatuiInlineRenderer,
    APPROVAL_MAX_WIDTH, MIN_WIDTH,
};

#[derive(Debug, Clone)]
pub struct ApprovalPanelModel<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub risk: &'a str,
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

        let width = self.width.clamp(MIN_WIDTH, APPROVAL_MAX_WIDTH);
        let height = approval_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_panel(model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn approval_panel_write_lines(&self, model: ApprovalPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_approval_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, APPROVAL_MAX_WIDTH);
        let height = approval_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_panel(model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_approval_panel_lines(&self, model: ApprovalPanelModel<'_>) -> Vec<String> {
        if is_command_approval_request(&model) {
            let command_rows = command_preview_rows(
                model.preview,
                self.content_width(),
                max_preview_rows(model.expanded),
            );
            let mut lines = vec![
                "Approval required".to_string(),
                command_request_heading(model.subject).to_string(),
            ];
            lines.extend(command_rows);
            if model.queue_total > 1 {
                let mut queue = format!(
                    "Queue: {}/{} pending",
                    model.queue_position, model.queue_total
                );
                if let Some(next) = model.next_label {
                    queue.push_str(&format!("; next {next}"));
                }
                lines.push(queue);
            }
            lines.push(approval_action_line(model.selected_action));
            if model.expanded {
                lines.push(
                    "Default: deny; approved command is rechecked by read-only broker.".to_string(),
                );
                lines.push(
                    "Keys: Left/Right select · Enter confirm · d details · Esc cancel".to_string(),
                );
            }
            return lines;
        }

        let mut lines = vec![
            "Approval required".to_string(),
            format!("{} · {} · {} risk", model.id, model.kind, model.risk),
            format!(
                "Queue: {} of {} pending",
                model.queue_position, model.queue_total
            ),
            format!("Subject: {}", model.subject),
            format!("{}: {}", model.preview_label, model.preview),
        ];
        if let Some(next) = model.next_label {
            lines.push(format!("Next: {next}"));
        }
        lines.push(approval_action_line(model.selected_action));
        if model.expanded {
            lines.push(
                "Policy: user approval is required before any executable tool request".to_string(),
            );
            lines.push(
                "Keys: ←/→ select · Enter confirm · d details · Esc/Ctrl+C cancel".to_string(),
            );
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
        let expanded_rows = if model.expanded { 2 } else { 0 };
        return 4 + command_rows + queue_rows + expanded_rows;
    }

    let preview_rows = wrapped_preview_rows(
        model.preview,
        content_width,
        max_preview_rows(model.expanded),
    )
    .len()
    .max(1) as u16;
    let next_rows = u16::from(model.next_label.is_some());
    let policy_rows = if model.expanded { 2 } else { 0 };
    7 + preview_rows + next_rows + policy_rows
}

fn render_approval_panel(model: ApprovalPanelModel<'_>, area: Rect, buffer: &mut Buffer) {
    if is_command_approval_request(&model) {
        render_command_tool_approval_panel(model, area, buffer);
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
            Span::styled(" Approval ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} ", model.id)),
        ]))
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
    let mut constraints = vec![
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(preview_height),
        Constraint::Length(next_height),
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
        Span::styled(format!("{} risk", model.risk), Style::default().fg(border)),
        Span::raw(format!(
            "  queue {}/{} pending",
            model.queue_position, model.queue_total
        )),
    ]))
    .render(chunks[0], buffer);

    Paragraph::new(Line::from(vec![
        Span::styled("Subject: ", Style::default().add_modifier(Modifier::BOLD)),
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
            Span::styled("Next: ", Style::default().fg(Color::DarkGray)),
            Span::raw(next.to_string()),
        ]))
        .render(chunks[4], buffer);
    }

    Paragraph::new(approval_action_spans(model.selected_action)).render(chunks[5], buffer);

    Paragraph::new(Line::from(vec![
        Span::styled("Keys: ", Style::default().fg(Color::DarkGray)),
        Span::raw("Left/Right select  Enter confirm  d details  Esc cancel"),
    ]))
    .render(chunks[6], buffer);

    if model.expanded {
        Paragraph::new(Text::from(vec![
            Line::from("Policy: user approval is required before any executable tool request."),
            Line::from("Only approved read-only Bash/shell tool requests may run in this MVP."),
        ]))
        .wrap(Wrap { trim: true })
        .render(chunks[7], buffer);
    }
}

fn render_command_tool_approval_panel(
    model: ApprovalPanelModel<'_>,
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
            Span::styled(" Approval ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} ", model.id)),
        ]))
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    block.render(area, buffer);
    let command_rows = command_preview_rows(
        model.preview,
        inner.width.saturating_sub(2) as usize,
        max_preview_rows(model.expanded),
    );
    let queue_height = u16::from(model.queue_total > 1);
    let mut constraints = vec![
        Constraint::Length(1),
        Constraint::Length(command_rows.len().max(1) as u16),
        Constraint::Length(queue_height),
        Constraint::Length(1),
    ];
    if model.expanded {
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Length(1));
    }
    let chunks = Layout::vertical(constraints).split(inner);
    let action_index = 3;
    let keys_index = 4;
    let policy_index = 5;

    Paragraph::new(Line::from(Span::styled(
        command_request_heading(model.subject),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )))
    .render(chunks[0], buffer);

    let command_lines = command_rows
        .into_iter()
        .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::White))))
        .collect::<Vec<_>>();
    Paragraph::new(Text::from(command_lines)).render(chunks[1], buffer);

    if model.queue_total > 1 {
        let mut queue = format!(
            "Queue: {}/{} pending",
            model.queue_position, model.queue_total
        );
        if let Some(next) = model.next_label {
            queue.push_str(&format!("; next {next}"));
        }
        Paragraph::new(Line::from(Span::styled(
            queue,
            Style::default().fg(Color::DarkGray),
        )))
        .render(chunks[2], buffer);
    }

    Paragraph::new(approval_action_spans(model.selected_action))
        .render(chunks[action_index], buffer);

    if model.expanded {
        Paragraph::new(Line::from(vec![
            Span::styled("Keys: ", Style::default().fg(Color::DarkGray)),
            Span::raw("Left/Right select  Enter confirm  d details  Esc cancel"),
        ]))
        .render(chunks[keys_index], buffer);
        Paragraph::new("Default: deny. Approved command is rechecked by read-only broker.")
            .wrap(Wrap { trim: true })
            .render(chunks[policy_index], buffer);
    }
}

fn approval_action_spans(selected: ApprovalPanelAction) -> Line<'static> {
    let mut spans = Vec::new();
    for (idx, descriptor) in APPROVAL_PANEL_ACTIONS.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(action_span(
            descriptor.label,
            descriptor.action,
            selected == descriptor.action,
        ));
    }
    Line::from(spans)
}

fn approval_action_line(selected: ApprovalPanelAction) -> String {
    APPROVAL_PANEL_ACTIONS
        .iter()
        .map(|descriptor| {
            if descriptor.action == selected {
                format!("[{}]", descriptor.label)
            } else {
                descriptor.label.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn action_span(label: &'static str, action: ApprovalPanelAction, selected: bool) -> Span<'static> {
    if selected {
        Span::styled(format!("> [ {label} ] "), selected_action_style(action))
    } else {
        Span::styled(format!("  [ {label} ] "), Style::default().fg(Color::Gray))
    }
}

fn selected_action_style(action: ApprovalPanelAction) -> Style {
    let background = match action {
        ApprovalPanelAction::Approve => Color::Green,
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

fn command_request_heading(subject: &str) -> &'static str {
    if subject.eq_ignore_ascii_case("tool shell") || subject.eq_ignore_ascii_case("shell command") {
        "Run shell command?"
    } else {
        "Run Bash command?"
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
