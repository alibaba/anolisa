use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, Paragraph, Widget, Wrap},
};

use super::{
    approval::{approval_content_width, wrapped_preview_rows},
    approval_receipt::receipt_preview_label,
    buffer_to_lines, buffer_to_styled_lines, RatatuiInlineRenderer, APPROVAL_MAX_WIDTH, MIN_WIDTH,
};

#[derive(Debug, Clone)]
pub struct ApprovalJournalEntryModel<'a> {
    pub id: &'a str,
    pub run_id: &'a str,
    pub source: &'a str,
    pub decision: &'a str,
    pub kind: &'a str,
    pub risk: &'a str,
    pub subject: &'a str,
    pub preview: &'a str,
}

#[derive(Debug, Clone)]
pub struct ApprovalJournalPanelModel<'a> {
    pub entries: &'a [ApprovalJournalEntryModel<'a>],
}

impl RatatuiInlineRenderer {
    pub fn write_approval_journal_panel<W: Write>(
        &self,
        output: &mut W,
        model: ApprovalJournalPanelModel<'_>,
    ) -> io::Result<usize> {
        let lines = self.approval_journal_panel_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn approval_journal_panel_lines(
        &self,
        model: ApprovalJournalPanelModel<'_>,
    ) -> Vec<String> {
        if self.plain {
            return self.plain_approval_journal_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, APPROVAL_MAX_WIDTH);
        let height = approval_journal_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_journal_panel(model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn approval_journal_panel_write_lines(
        &self,
        model: ApprovalJournalPanelModel<'_>,
    ) -> Vec<String> {
        if self.plain {
            return self.plain_approval_journal_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, APPROVAL_MAX_WIDTH);
        let height = approval_journal_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_approval_journal_panel(model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_approval_journal_panel_lines(
        &self,
        model: ApprovalJournalPanelModel<'_>,
    ) -> Vec<String> {
        if model.entries.is_empty() {
            return vec![
                "Approval journal".to_string(),
                "No approval decisions recorded in this shell session.".to_string(),
            ];
        }

        let mut lines = vec![format!(
            "Approval journal - {} decisions",
            model.entries.len()
        )];
        for entry in model.entries {
            lines.push(format!(
                "{} {} - {} - {} risk",
                entry.id, entry.decision, entry.kind, entry.risk
            ));
            lines.push(format!("  Source: {}  Run: {}", entry.source, entry.run_id));
            lines.push(format!("  Subject: {}", entry.subject));
            lines.push(format!(
                "  {}: {}",
                receipt_preview_label(entry.subject),
                entry.preview
            ));
        }
        lines
    }
}

fn approval_journal_panel_height(model: &ApprovalJournalPanelModel<'_>, width: u16) -> u16 {
    if model.entries.is_empty() {
        return 4;
    }

    let content_width = approval_content_width(width);
    let row_count = model
        .entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let separator_rows = u16::from(idx > 0);
            let preview_rows = wrapped_preview_rows(
                &format!(
                    "{}: {}",
                    receipt_preview_label(entry.subject),
                    entry.preview
                ),
                content_width,
                2,
            )
            .len()
            .max(1) as u16;
            separator_rows + 3 + preview_rows
        })
        .sum::<u16>();
    2 + row_count
}

fn render_approval_journal_panel(
    model: ApprovalJournalPanelModel<'_>,
    area: Rect,
    buffer: &mut Buffer,
) {
    let block = Block::bordered()
        .padding(Padding::horizontal(1))
        .title(Line::from(vec![
            Span::styled(
                " Approval journal ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} decisions ", model.entries.len())),
        ]))
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    block.render(area, buffer);

    if model.entries.is_empty() {
        Paragraph::new("No approval decisions recorded in this shell session.")
            .wrap(Wrap { trim: true })
            .render(inner, buffer);
        return;
    }

    let mut lines = Vec::new();
    for (idx, entry) in model.entries.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::from(""));
        }
        let decision_style = if entry.decision == "approved" {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(vec![
            Span::styled(
                entry.id.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(entry.decision.to_string(), decision_style),
            Span::raw("  "),
            Span::styled(entry.kind.to_string(), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled(
                format!("{} risk", entry.risk),
                Style::default().fg(Color::Yellow),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Source: ", Style::default().fg(Color::DarkGray)),
            Span::raw(entry.source.to_string()),
            Span::raw("  "),
            Span::styled("Run: ", Style::default().fg(Color::DarkGray)),
            Span::raw(entry.run_id.to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Subject: ", Style::default().fg(Color::DarkGray)),
            Span::raw(entry.subject.to_string()),
        ]));
        for row in wrapped_preview_rows(
            &format!(
                "{}: {}",
                receipt_preview_label(entry.subject),
                entry.preview
            ),
            inner.width.saturating_sub(2) as usize,
            2,
        ) {
            lines.push(Line::from(row));
        }
    }

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .render(inner, buffer);
}
