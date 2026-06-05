use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::ROUNDED,
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget, Wrap},
};

use super::{buffer_to_lines, buffer_to_styled_lines, wrap_plain_line, RatatuiInlineRenderer};

#[derive(Debug, Clone)]
pub struct ConsultationCardModel {
    pub hook_id: String,
    pub details_id: String,
    pub severity: String,
    pub confidence: String,
    pub display_reason: String,
    pub title: String,
    pub suggestion: String,
}

impl RatatuiInlineRenderer {
    pub fn write_consultation_card<W: Write>(
        &self,
        output: &mut W,
        model: &ConsultationCardModel,
    ) -> io::Result<usize> {
        let lines = self.consultation_card_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn consultation_card_lines(&self, model: &ConsultationCardModel) -> Vec<String> {
        if self.plain {
            return self.plain_consultation_card_lines(model);
        }

        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let height = consultation_card_height(&i18n, model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_consultation_card(&i18n, model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn consultation_card_write_lines(&self, model: &ConsultationCardModel) -> Vec<String> {
        if self.plain {
            return self.plain_consultation_card_lines(model);
        }

        let i18n = self.i18n();
        let width = self.panel_standard_width();
        let height = consultation_card_height(&i18n, model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_consultation_card(&i18n, model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_consultation_card_lines(&self, model: &ConsultationCardModel) -> Vec<String> {
        let i18n = self.i18n();
        let content_width = self.content_width();
        let mut lines = vec![format!(
            "{}: {} [{}]:",
            i18n.t(crate::MessageId::HookConsultationHookLabel),
            model.hook_id,
            model.severity
        )];
        lines.extend(wrap_plain_line(
            &format!("  {}", model.title),
            content_width,
        ));
        lines.extend(wrap_plain_line(
            &format!("  {}", consultation_card_meta(&i18n, model)),
            content_width,
        ));
        lines.extend(wrap_plain_line(
            &format!("  {}", model.suggestion),
            content_width,
        ));
        lines.push(format!(
            "  [{}] [{}] [Details] {}",
            i18n.t(crate::MessageId::HookConsultationAnalyzeAction),
            i18n.t(crate::MessageId::HookConsultationIgnoreAction),
            model.details_id
        ));
        lines
    }
}

fn consultation_card_height(i18n: &crate::I18n, model: &ConsultationCardModel, width: u16) -> u16 {
    let content_width = panel_content_width(width);
    let title_rows = wrapped_row_count(&model.title, content_width);
    let meta_rows = wrapped_row_count(&consultation_card_meta(i18n, model), content_width);
    let suggestion_rows = wrapped_row_count(&model.suggestion, content_width);
    let actions_rows = 1;
    title_rows + meta_rows + suggestion_rows + actions_rows + 2
}

fn render_consultation_card(
    i18n: &crate::I18n,
    model: &ConsultationCardModel,
    area: Rect,
    buffer: &mut Buffer,
) {
    let title_text = format!(
        " {}: {} ",
        i18n.t(crate::MessageId::HookConsultationHookLabel),
        model.hook_id
    );
    let severity_text = format!(" {} ", model.severity);

    let block = Block::bordered()
        .title(Line::from(vec![
            Span::styled(title_text, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("─── {} ", severity_text.trim()),
                Style::default()
                    .fg(severity_color(&model.severity))
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .border_set(ROUNDED)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    block.render(area, buffer);

    let content_width = inner.width as usize;
    let title_rows = wrapped_row_count(&model.title, content_width);
    let meta_text = consultation_card_meta(i18n, model);
    let meta_rows = wrapped_row_count(&meta_text, content_width);
    let suggestion_rows = wrapped_row_count(&model.suggestion, content_width);
    let actions_rows = 1u16;
    let chunks = Layout::vertical(vec![
        Constraint::Length(title_rows),
        Constraint::Length(meta_rows),
        Constraint::Length(suggestion_rows),
        Constraint::Length(actions_rows),
    ])
    .split(inner);

    Paragraph::new(model.title.as_str())
        .wrap(Wrap { trim: true })
        .render(chunks[0], buffer);

    Paragraph::new(meta_text)
        .style(Style::default().fg(Color::DarkGray))
        .wrap(Wrap { trim: true })
        .render(chunks[1], buffer);

    Paragraph::new(model.suggestion.as_str())
        .style(Style::default().fg(Color::Gray))
        .wrap(Wrap { trim: true })
        .render(chunks[2], buffer);

    Paragraph::new(Line::from(vec![
        Span::styled(
            format!(
                "[{}]",
                i18n.t(crate::MessageId::HookConsultationAnalyzeAction)
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!(
                "[{}]",
                i18n.t(crate::MessageId::HookConsultationIgnoreAction)
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(format!(" [Details] {}", model.details_id)),
    ]))
    .render(chunks[3], buffer);
}

fn consultation_card_meta(i18n: &crate::I18n, model: &ConsultationCardModel) -> String {
    i18n.format(
        crate::MessageId::HookConsultationConfidenceReasonLine,
        &[
            ("confidence", model.confidence.as_str()),
            ("reason", model.display_reason.as_str()),
        ],
    )
}

fn severity_color(severity: &str) -> Color {
    match severity.to_lowercase().as_str() {
        "critical" | "error" => Color::Red,
        "warning" | "warn" => Color::Yellow,
        "info" => Color::Cyan,
        _ => Color::White,
    }
}

fn panel_content_width(width: u16) -> usize {
    width.saturating_sub(2).max(20) as usize
}

fn wrapped_row_count(text: &str, width: usize) -> u16 {
    wrap_plain_line(text, width).len().max(1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_consultation_card_includes_confidence_and_reason() {
        let renderer = RatatuiInlineRenderer::plain_with_width(80);
        let lines = renderer.consultation_card_lines(&ConsultationCardModel {
            hook_id: "memory-pressure".to_string(),
            details_id: "hook-cmd-1-memory-pressure".to_string(),
            severity: "critical".to_string(),
            confidence: "high".to_string(),
            display_reason: "allowed".to_string(),
            title: "Available memory is low".to_string(),
            suggestion: "Analyze the output before taking action.".to_string(),
        });

        let rendered = lines.join("\n");
        assert!(rendered.contains("Confidence: high; reason: allowed"));
        assert!(rendered.contains("[Analyze] [Ignore] [Details] hook-cmd-1-memory-pressure"));
    }

    #[test]
    fn consultation_card_uses_zh_catalog_labels() {
        let renderer =
            RatatuiInlineRenderer::plain_with_width(80).with_language(crate::Language::ZhCn);
        let lines = renderer.consultation_card_lines(&ConsultationCardModel {
            hook_id: "memory-pressure".to_string(),
            details_id: "hook-cmd-1-memory-pressure".to_string(),
            severity: "critical".to_string(),
            confidence: "high".to_string(),
            display_reason: "allowed".to_string(),
            title: "Available memory is low".to_string(),
            suggestion: "Analyze the output before taking action.".to_string(),
        });

        let rendered = lines.join("\n");
        assert!(rendered.contains("Hook: memory-pressure [critical]:"));
        assert!(rendered.contains("置信度: high; 原因: allowed"));
        assert!(rendered.contains("[分析] [忽略] [Details] hook-cmd-1-memory-pressure"));
        assert!(!rendered.contains("Confidence:"), "{rendered}");
        assert!(!rendered.contains("[Analyze]"), "{rendered}");
    }
}
