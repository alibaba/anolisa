use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, Paragraph, Widget, Wrap},
};

use super::{
    buffer_to_lines, buffer_to_styled_lines, wrap_plain_line, RatatuiInlineRenderer,
    ACTIVITY_MAX_WIDTH, MIN_WIDTH,
};

#[derive(Debug, Clone)]
pub struct ActivityRowModel<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub status: &'a str,
    pub subject: &'a str,
    pub summary: &'a str,
}

#[derive(Debug, Clone)]
pub struct ActivityPanelModel<'a> {
    pub rows: Vec<ActivityRowModel<'a>>,
}

#[derive(Debug, Clone)]
pub struct ActivityDetailsPanelModel<'a> {
    pub id: &'a str,
    pub run_id: &'a str,
    pub kind: &'a str,
    pub status: &'a str,
    pub subject: &'a str,
    pub summary: &'a str,
    pub detail: &'a str,
}

impl RatatuiInlineRenderer {
    pub fn write_activity_panel<W: Write>(
        &self,
        output: &mut W,
        model: ActivityPanelModel<'_>,
    ) -> io::Result<usize> {
        let lines = self.activity_panel_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn activity_panel_lines(&self, model: ActivityPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_activity_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, ACTIVITY_MAX_WIDTH);
        let height = activity_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_panel(model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn activity_panel_write_lines(&self, model: ActivityPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_activity_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, ACTIVITY_MAX_WIDTH);
        let height = activity_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_panel(model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_activity_panel_lines(&self, model: ActivityPanelModel<'_>) -> Vec<String> {
        let width = self.width.clamp(MIN_WIDTH, ACTIVITY_MAX_WIDTH);
        let content_width = activity_panel_content_width(width);
        let mut lines = vec!["Activity:".to_string()];
        lines.extend(
            model
                .rows
                .into_iter()
                .flat_map(|row| wrap_plain_line(&activity_row_text(&row), content_width)),
        );
        lines
    }

    pub fn write_activity_details_panel<W: Write>(
        &self,
        output: &mut W,
        model: ActivityDetailsPanelModel<'_>,
    ) -> io::Result<usize> {
        let lines = self.activity_details_panel_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn activity_details_panel_lines(
        &self,
        model: ActivityDetailsPanelModel<'_>,
    ) -> Vec<String> {
        if self.plain {
            return self.plain_activity_details_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, ACTIVITY_MAX_WIDTH);
        let height = activity_details_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_details_panel(model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn activity_details_panel_write_lines(
        &self,
        model: ActivityDetailsPanelModel<'_>,
    ) -> Vec<String> {
        if self.plain {
            return self.plain_activity_details_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, ACTIVITY_MAX_WIDTH);
        let height = activity_details_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_details_panel(model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_activity_details_panel_lines(
        &self,
        model: ActivityDetailsPanelModel<'_>,
    ) -> Vec<String> {
        let width = self.width.clamp(MIN_WIDTH, ACTIVITY_MAX_WIDTH);
        let content_width = panel_content_width(width);
        let mut lines = vec![format!("Activity details {}", model.id)];
        lines.extend(wrap_plain_line(
            &format!(
                "{} - {} - {}",
                model.kind,
                activity_summary(model.status, model.summary),
                model.subject
            ),
            content_width,
        ));
        lines.extend(wrap_plain_line(
            &format!("Run: {}", model.run_id),
            content_width,
        ));
        lines.push("Detail:".to_string());
        for detail_line in model.detail.lines() {
            lines.extend(wrap_plain_line(detail_line, content_width));
        }
        lines
    }
}

fn activity_panel_height(model: &ActivityPanelModel<'_>, width: u16) -> u16 {
    let content_width = activity_panel_content_width(width);
    activity_row_heights(model, content_width)
        .into_iter()
        .sum::<u16>()
        .max(1)
        + 2
}

fn render_activity_panel(model: ActivityPanelModel<'_>, area: Rect, buffer: &mut Buffer) {
    let block = Block::bordered()
        .padding(Padding::horizontal(1))
        .title(Line::from(Span::styled(
            " Activity ",
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    block.render(area, buffer);

    let row_constraints = activity_row_heights(&model, inner.width as usize)
        .into_iter()
        .map(Constraint::Length)
        .collect::<Vec<_>>();
    let chunks = Layout::vertical(row_constraints).split(inner);

    for (idx, row) in model.rows.into_iter().enumerate() {
        let Some(area) = chunks.get(idx).copied() else {
            break;
        };
        Paragraph::new(Text::from(styled_activity_row_line(&row)))
            .wrap(Wrap { trim: true })
            .render(area, buffer);
    }
}

fn activity_summary(status: &str, summary: &str) -> String {
    if status.is_empty() || status == "captured" || summary.contains(status) {
        summary.to_string()
    } else {
        format!("{status} · {summary}")
    }
}

fn activity_row_text(row: &ActivityRowModel<'_>) -> String {
    let summary = activity_summary(row.status, row.summary);
    match row.kind {
        "skill" => {
            let status = if row.status.is_empty() {
                "updated"
            } else {
                row.status
            };
            if row.subject.is_empty() {
                format!("Skill {status}")
            } else {
                format!("Skill {status}: {}", row.subject)
            }
        }
        "output" => format!("Tool output: {summary}"),
        "tool" => {
            if summary.is_empty() || summary == row.status {
                format!("Tool {}", row.status)
            } else {
                let status_prefix = format!("{} · ", row.status);
                let summary = summary.strip_prefix(&status_prefix).unwrap_or(&summary);
                format!("Tool {}: {summary}", row.status)
            }
        }
        _ => {
            if let Some(subject) = activity_subject_suffix(row) {
                format!("{}: {} {}", row.kind, summary, subject)
            } else {
                format!("{}: {}", row.kind, summary)
            }
        }
    }
}

fn styled_activity_row_line(row: &ActivityRowModel<'_>) -> Line<'static> {
    let text = activity_row_text(row);
    let Some((label, rest)) = text.split_once(':') else {
        return Line::from(Span::styled(text, Style::default().fg(Color::White)));
    };
    Line::from(vec![
        Span::styled(
            format!("{label}:"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(rest.to_string()),
    ])
}

fn activity_row_heights(model: &ActivityPanelModel<'_>, width: usize) -> Vec<u16> {
    if model.rows.is_empty() {
        return vec![1];
    }

    model
        .rows
        .iter()
        .map(|row| wrap_plain_line(&activity_row_text(row), width).len().max(1) as u16)
        .collect()
}

fn activity_subject_suffix<'a>(row: &ActivityRowModel<'a>) -> Option<&'a str> {
    if row.subject.is_empty()
        || row.subject == row.id
        || row.summary.contains(row.subject)
        || (row.kind == "output" && row.subject.starts_with("tool-"))
    {
        None
    } else {
        Some(row.subject)
    }
}

fn activity_details_panel_height(model: &ActivityDetailsPanelModel<'_>, width: u16) -> u16 {
    activity_details_lines(model, panel_content_width(width))
        .len()
        .max(1) as u16
        + 2
}

fn render_activity_details_panel(
    model: ActivityDetailsPanelModel<'_>,
    area: Rect,
    buffer: &mut Buffer,
) {
    let block = Block::bordered()
        .title(Line::from(vec![
            Span::styled(
                " Activity details ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} ", model.id)),
        ]))
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    block.render(area, buffer);

    Paragraph::new(Text::from(activity_details_lines(
        &model,
        inner.width as usize,
    )))
    .render(inner, buffer);
}

fn activity_details_lines(
    model: &ActivityDetailsPanelModel<'_>,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_wrapped_line(
        &mut lines,
        &format!(
            "{} - {} - {}",
            model.kind,
            activity_summary(model.status, model.summary),
            model.subject
        ),
        width,
    );
    push_wrapped_line(&mut lines, &format!("Run: {}", model.run_id), width);
    push_wrapped_line(&mut lines, "Detail:", width);
    for detail_line in model.detail.lines() {
        push_wrapped_line(&mut lines, detail_line, width);
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn push_wrapped_line(lines: &mut Vec<Line<'static>>, text: &str, width: usize) {
    lines.extend(wrap_plain_line(text, width).into_iter().map(Line::from));
}

fn panel_content_width(width: u16) -> usize {
    width.saturating_sub(2).max(20) as usize
}

fn activity_panel_content_width(width: u16) -> usize {
    width.saturating_sub(4).max(20) as usize
}
