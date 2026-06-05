use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, Paragraph, Widget, Wrap},
};

use super::{
    approval::{approval_content_width, wrapped_preview_rows},
    buffer_to_lines, buffer_to_styled_lines, RatatuiInlineRenderer, APPROVAL_MAX_WIDTH, MIN_WIDTH,
};

#[derive(Debug, Clone)]
pub struct ApprovalDetailsPanelModel<'a> {
    pub id: &'a str,
    pub run_id: &'a str,
    pub source: &'a str,
    pub kind: &'a str,
    pub status: &'a str,
    pub risk: &'a str,
    pub subject: &'a str,
    pub preview_label: &'a str,
    pub preview: &'a str,
}

impl RatatuiInlineRenderer {
    pub fn write_approval_details_panel<W: Write>(
        &self,
        output: &mut W,
        model: ApprovalDetailsPanelModel<'_>,
    ) -> io::Result<usize> {
        let lines = self.approval_details_panel_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn approval_details_panel_lines(
        &self,
        model: ApprovalDetailsPanelModel<'_>,
    ) -> Vec<String> {
        if self.plain {
            return self.plain_approval_details_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, APPROVAL_MAX_WIDTH);
        let height = approval_details_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_details_panel(model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn approval_details_panel_write_lines(
        &self,
        model: ApprovalDetailsPanelModel<'_>,
    ) -> Vec<String> {
        if self.plain {
            return self.plain_approval_details_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, APPROVAL_MAX_WIDTH);
        let height = approval_details_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_details_panel(model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_approval_details_panel_lines(
        &self,
        model: ApprovalDetailsPanelModel<'_>,
    ) -> Vec<String> {
        vec![
            format!("Approval details {}", model.id),
            format!("{} - {} - {} risk", model.kind, model.status, model.risk),
            format!("Source: {}  Run: {}", model.source, model.run_id),
            "Default: deny".to_string(),
            format!("Request: {}", user_facing_subject(model.subject)),
            format!("{}: {}", user_facing_preview_label(&model), model.preview),
            "Policy: user approval is required before any executable tool request.".to_string(),
        ]
    }
}

fn approval_details_panel_height(model: &ApprovalDetailsPanelModel<'_>, width: u16) -> u16 {
    let content_width = approval_content_width(width);
    let preview_rows = wrapped_preview_rows(model.preview, content_width, 6)
        .len()
        .max(1) as u16;
    8 + preview_rows
}

fn render_approval_details_panel(
    model: ApprovalDetailsPanelModel<'_>,
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
                " Approval details ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} ", model.id)),
        ]))
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    block.render(area, buffer);

    let preview_rows =
        wrapped_preview_rows(model.preview, inner.width.saturating_sub(2) as usize, 6);
    let chunks = Layout::vertical(vec![
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(preview_rows.len().max(1) as u16),
        Constraint::Length(1),
    ])
    .split(inner);

    Paragraph::new(Line::from(vec![
        Span::styled(model.kind.to_string(), Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::raw(model.status.to_string()),
        Span::raw("  "),
        Span::styled(format!("{} risk", model.risk), Style::default().fg(border)),
    ]))
    .render(chunks[0], buffer);
    Paragraph::new(Line::from(vec![
        Span::styled("Source: ", Style::default().fg(Color::DarkGray)),
        Span::raw(model.source.to_string()),
        Span::raw("  "),
        Span::styled("Run: ", Style::default().fg(Color::DarkGray)),
        Span::raw(model.run_id.to_string()),
    ]))
    .render(chunks[1], buffer);
    Paragraph::new("Default: deny").render(chunks[2], buffer);
    Paragraph::new(Line::from(vec![
        Span::styled("Request: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(user_facing_subject(model.subject)),
    ]))
    .render(chunks[3], buffer);
    Paragraph::new(Line::from(Span::styled(
        format!("{}:", user_facing_preview_label(&model)),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )))
    .render(chunks[4], buffer);
    Paragraph::new(Text::from(
        preview_rows
            .into_iter()
            .map(|line| Line::from(Span::raw(line)))
            .collect::<Vec<_>>(),
    ))
    .render(chunks[5], buffer);
    Paragraph::new("Policy: user approval is required before any executable tool request.")
        .wrap(Wrap { trim: true })
        .render(chunks[6], buffer);
}

fn user_facing_subject(subject: &str) -> String {
    let subject = subject.trim();
    if subject.eq_ignore_ascii_case("tool Bash") || subject.eq_ignore_ascii_case("tool shell") {
        "Bash command".to_string()
    } else if subject.eq_ignore_ascii_case("shell command") {
        "Shell command".to_string()
    } else if let Some(tool) = subject.strip_prefix("tool ") {
        format!("{tool} tool")
    } else {
        subject.to_string()
    }
}

fn user_facing_preview_label(model: &ApprovalDetailsPanelModel<'_>) -> String {
    let subject = model.subject.to_ascii_lowercase();
    if subject.contains("bash") || subject.contains("shell") {
        "Command".to_string()
    } else if model.preview_label.eq_ignore_ascii_case("Tool input") {
        "Input".to_string()
    } else {
        model.preview_label.to_string()
    }
}
