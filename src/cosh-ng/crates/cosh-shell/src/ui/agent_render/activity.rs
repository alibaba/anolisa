use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::ROUNDED,
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, Paragraph, Widget, Wrap},
};

use super::{buffer_to_lines, buffer_to_styled_lines, wrap_plain_line, RatatuiInlineRenderer};

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

        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let height = activity_panel_height(&i18n, &model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_panel(&i18n, model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn activity_panel_write_lines(&self, model: ActivityPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_activity_panel_lines(model);
        }

        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let height = activity_panel_height(&i18n, &model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_panel(&i18n, model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_activity_panel_lines(&self, model: ActivityPanelModel<'_>) -> Vec<String> {
        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let content_width = activity_panel_content_width(width);
        let mut lines = vec![format!("{}:", i18n.t(crate::MessageId::ActivityTitle))];
        lines.extend(
            model
                .rows
                .into_iter()
                .flat_map(|row| wrap_plain_line(&activity_row_text(&i18n, &row), content_width)),
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

        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let height = activity_details_panel_height(&i18n, &model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_details_panel(&i18n, model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn activity_details_panel_write_lines(
        &self,
        model: ActivityDetailsPanelModel<'_>,
    ) -> Vec<String> {
        if self.plain {
            return self.plain_activity_details_panel_lines(model);
        }

        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let height = activity_details_panel_height(&i18n, &model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_activity_details_panel(&i18n, model, area, &mut buffer);
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
        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let content_width = panel_content_width(width);
        let mut lines = vec![format!(
            "{} {}",
            i18n.t(crate::MessageId::ActivityDetailsTitle),
            model.id
        )];
        lines.extend(wrap_plain_line(
            &format!(
                "{} - {} - {}",
                activity_kind_label(&i18n, model.kind),
                activity_summary(&i18n, model.status, model.summary),
                model.subject
            ),
            content_width,
        ));
        lines.extend(wrap_plain_line(
            &format!(
                "{}: {}",
                i18n.t(crate::MessageId::ActivityRunLabel),
                model.run_id
            ),
            content_width,
        ));
        lines.push(format!(
            "{}:",
            i18n.t(crate::MessageId::ActivityDetailLabel)
        ));
        for detail_line in model.detail.lines() {
            lines.extend(wrap_plain_line(detail_line, content_width));
        }
        lines
    }
}

fn activity_panel_height(i18n: &crate::I18n, model: &ActivityPanelModel<'_>, width: u16) -> u16 {
    let content_width = activity_panel_content_width(width);
    activity_row_heights(i18n, model, content_width)
        .into_iter()
        .sum::<u16>()
        .max(1)
        + 2
}

fn render_activity_panel(
    i18n: &crate::I18n,
    model: ActivityPanelModel<'_>,
    area: Rect,
    buffer: &mut Buffer,
) {
    let block = Block::bordered()
        .padding(Padding::horizontal(1))
        .title(Line::from(Span::styled(
            format!(" {} ", i18n.t(crate::MessageId::ActivityTitle)),
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .border_set(ROUNDED)
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    block.render(area, buffer);

    let row_constraints = activity_row_heights(i18n, &model, inner.width as usize)
        .into_iter()
        .map(Constraint::Length)
        .collect::<Vec<_>>();
    let chunks = Layout::vertical(row_constraints).split(inner);

    for (idx, row) in model.rows.into_iter().enumerate() {
        let Some(area) = chunks.get(idx).copied() else {
            break;
        };
        Paragraph::new(Text::from(styled_activity_row_line(i18n, &row)))
            .wrap(Wrap { trim: true })
            .render(area, buffer);
    }
}

fn activity_summary(i18n: &crate::I18n, status: &str, summary: &str) -> String {
    if status.is_empty() || status == "captured" || summary.contains(status) {
        summary.to_string()
    } else {
        format!("{} · {summary}", activity_status_label(i18n, status))
    }
}

fn activity_row_text(i18n: &crate::I18n, row: &ActivityRowModel<'_>) -> String {
    let summary = activity_summary(i18n, row.status, row.summary);
    match row.kind {
        "skill" => {
            let status = if row.status.is_empty() {
                i18n.t(crate::MessageId::ActivitySkillUpdatedStatus)
            } else {
                activity_status_label(i18n, row.status)
            };
            if row.subject.is_empty() {
                format!("{} {status}", i18n.t(crate::MessageId::ActivitySkillLabel))
            } else {
                format!(
                    "{} {status}: {}",
                    i18n.t(crate::MessageId::ActivitySkillLabel),
                    row.subject
                )
            }
        }
        "output" => format!(
            "{}: {summary}",
            i18n.t(crate::MessageId::ActivityToolOutputLabel)
        ),
        "tool" => {
            if summary.is_empty() || summary == row.status {
                format!(
                    "{} {}",
                    i18n.t(crate::MessageId::ActivityToolLabel),
                    activity_status_label(i18n, row.status)
                )
            } else {
                let status = activity_status_label(i18n, row.status);
                let status_prefix = format!("{status} · ");
                let summary = summary.strip_prefix(&status_prefix).unwrap_or(&summary);
                format!(
                    "{} {}: {summary}",
                    i18n.t(crate::MessageId::ActivityToolLabel),
                    status
                )
            }
        }
        _ => {
            let kind = activity_kind_label(i18n, row.kind);
            if let Some(subject) = activity_subject_suffix(row) {
                format!("{kind}: {summary} {subject}")
            } else {
                format!("{kind}: {summary}")
            }
        }
    }
}

fn activity_kind_label(i18n: &crate::I18n, kind: &str) -> String {
    match kind {
        "skill" => i18n.t(crate::MessageId::ActivitySkillLabel).to_string(),
        "output" => i18n
            .t(crate::MessageId::ActivityToolOutputLabel)
            .to_string(),
        "tool" => i18n.t(crate::MessageId::ActivityToolLabel).to_string(),
        "shell" => i18n.t(crate::MessageId::ActivityShellLabel).to_string(),
        _ => kind.to_string(),
    }
}

fn activity_status_label<'a>(i18n: &crate::I18n, status: &'a str) -> &'a str {
    match status {
        "loading" => i18n.t(crate::MessageId::ActivityStatusLoading),
        "loaded" => i18n.t(crate::MessageId::ActivityStatusLoaded),
        "failed" => i18n.t(crate::MessageId::ActivityStatusFailed),
        "called" => i18n.t(crate::MessageId::ActivityStatusCalled),
        "requested" => i18n.t(crate::MessageId::ActivityStatusRequested),
        "captured" => i18n.t(crate::MessageId::ActivityStatusCaptured),
        "completed" => i18n.t(crate::MessageId::ActivityStatusCompleted),
        "error" => i18n.t(crate::MessageId::ActivityStatusError),
        "interrupted" => i18n.t(crate::MessageId::ActivityStatusInterrupted),
        _ => status,
    }
}

fn styled_activity_row_line(i18n: &crate::I18n, row: &ActivityRowModel<'_>) -> Line<'static> {
    let text = activity_row_text(i18n, row);
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

fn activity_row_heights(
    i18n: &crate::I18n,
    model: &ActivityPanelModel<'_>,
    width: usize,
) -> Vec<u16> {
    if model.rows.is_empty() {
        return vec![1];
    }

    model
        .rows
        .iter()
        .map(|row| {
            wrap_plain_line(&activity_row_text(i18n, row), width)
                .len()
                .max(1) as u16
        })
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

fn activity_details_panel_height(
    i18n: &crate::I18n,
    model: &ActivityDetailsPanelModel<'_>,
    width: u16,
) -> u16 {
    activity_details_lines(i18n, model, panel_content_width(width))
        .len()
        .max(1) as u16
        + 2
}

fn render_activity_details_panel(
    i18n: &crate::I18n,
    model: ActivityDetailsPanelModel<'_>,
    area: Rect,
    buffer: &mut Buffer,
) {
    let block = Block::bordered()
        .title(Line::from(vec![
            Span::styled(
                format!(" {} ", i18n.t(crate::MessageId::ActivityDetailsTitle)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} ", model.id)),
        ]))
        .border_set(ROUNDED)
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    block.render(area, buffer);

    Paragraph::new(Text::from(activity_details_lines(
        i18n,
        &model,
        inner.width as usize,
    )))
    .render(inner, buffer);
}

fn activity_details_lines(
    i18n: &crate::I18n,
    model: &ActivityDetailsPanelModel<'_>,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_wrapped_line(
        &mut lines,
        &format!(
            "{} - {} - {}",
            activity_kind_label(i18n, model.kind),
            activity_summary(i18n, model.status, model.summary),
            model.subject
        ),
        width,
    );
    push_wrapped_line(
        &mut lines,
        &format!(
            "{}: {}",
            i18n.t(crate::MessageId::ActivityRunLabel),
            model.run_id
        ),
        width,
    );
    push_wrapped_line(
        &mut lines,
        &format!("{}:", i18n.t(crate::MessageId::ActivityDetailLabel)),
        width,
    );
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
