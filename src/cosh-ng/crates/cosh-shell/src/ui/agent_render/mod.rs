use std::io::{self, IsTerminal, Write};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, BorderType, Paragraph, Widget, Wrap},
};

use crate::types::{AgentEvent, GovernedEvent};

mod actions;
mod activity;
mod approval;
mod approval_details;
mod approval_journal;
mod approval_receipt;
mod card;
mod consultation;
mod markdown;
mod notice;
mod question;
mod recommendation;
mod status;
mod stream;
mod wrap;

pub use actions::{
    approval_action_at, approval_action_index, ApprovalActionDescriptor, ApprovalPanelAction,
    APPROVAL_PANEL_ACTIONS,
};
pub use activity::{ActivityDetailsPanelModel, ActivityPanelModel, ActivityRowModel};
pub use approval::{ApprovalPanelModel, HookWarningView};
pub(crate) use approval::hook_warning_icon;
pub use approval_details::{ApprovalDetailsPanelModel, CommandAssessmentSummaryModel};
pub use approval_journal::{ApprovalJournalEntryModel, ApprovalJournalPanelModel};
pub use approval_receipt::ApprovalReceiptPanelModel;
pub use consultation::ConsultationCardModel;
use markdown::MarkdownRenderModel;
pub use notice::NoticePanelModel;
pub use question::{QuestionAnswerPanelModel, QuestionPanelModel};
pub use recommendation::{RecommendationActionPanelModel, RecommendationPanelModel};
pub use status::AgentStatusAnimation;
pub use stream::{MarkdownStreamBlock, StreamingAgentBlock};
use wrap::{
    char_width, compact_rendered_lines, display_width, line_to_string, strip_ansi_escape,
    wrap_plain_line,
};

const DEFAULT_WIDTH: u16 = 100;
const MIN_WIDTH: u16 = 40;
const MAX_WIDTH: u16 = 160;

#[derive(Debug, Clone)]
pub struct RatatuiInlineRenderer {
    width: u16,
    plain: bool,
    styled: bool,
    language: crate::Language,
}

impl RatatuiInlineRenderer {
    pub fn for_terminal() -> Self {
        let width = if let Some(width) = configured_terminal_width() {
            width
        } else if std::io::stdout().is_terminal() {
            ratatui::crossterm::terminal::size()
                .map(|(cols, _)| cols)
                .unwrap_or(DEFAULT_WIDTH)
                .clamp(MIN_WIDTH, 200)
        } else {
            DEFAULT_WIDTH
        };
        Self {
            width,
            plain: plain_output_requested(),
            styled: std::io::stdout().is_terminal(),
            language: crate::Language::EnUs,
        }
    }

    pub fn with_width(width: u16) -> Self {
        Self {
            width: width.clamp(20, 240),
            plain: false,
            styled: false,
            language: crate::Language::EnUs,
        }
    }

    pub fn plain_with_width(width: u16) -> Self {
        Self {
            width: width.clamp(20, 240),
            plain: true,
            styled: false,
            language: crate::Language::EnUs,
        }
    }

    pub fn with_language(mut self, language: crate::Language) -> Self {
        self.language = language;
        self
    }

    fn i18n(&self) -> crate::I18n {
        crate::I18n::new(self.language)
    }

    pub fn governed_event_lines(&self, governed_events: &[GovernedEvent]) -> Vec<String> {
        let i18n = self.i18n();
        self.render_lines(
            lines_from_governed_events(governed_events, &i18n),
            self.content_width(),
        )
    }

    pub fn write_governed_events<W: Write>(
        &self,
        output: &mut W,
        governed_events: &[GovernedEvent],
    ) -> io::Result<()> {
        self.write_block(
            output,
            self.i18n().t(crate::MessageId::AgentGovernanceTitle),
            self.governed_event_lines(governed_events),
            None,
        )
    }

    pub fn write_loading<W: Write>(&self, output: &mut W) -> io::Result<()> {
        self.write_loading_text(output, self.i18n().t(crate::MessageId::AgentThinking))
    }

    pub fn write_loading_text<W: Write>(&self, output: &mut W, text: &str) -> io::Result<()> {
        self.write_block(
            output,
            self.i18n().t(crate::MessageId::AgentResponseTitle),
            vec![text.to_string()],
            None,
        )
    }

    pub fn status_animation(&self) -> AgentStatusAnimation {
        AgentStatusAnimation::new(self.supports_status_animation())
    }

    pub fn stream_agent(&self) -> StreamingAgentBlock {
        StreamingAgentBlock::new(
            self.content_width(),
            self.plain,
            self.i18n().t(crate::MessageId::AgentResponseTitle),
        )
    }

    pub fn stream_markdown_agent(&self) -> MarkdownStreamBlock {
        MarkdownStreamBlock::new(self.clone())
    }

    pub fn write_markdown_text<W: Write>(&self, output: &mut W, text: &str) -> io::Result<()> {
        self.write_agent_response(output, text, None)
    }

    pub fn write_agent_response<W: Write>(
        &self,
        output: &mut W,
        text: &str,
        footer: Option<&str>,
    ) -> io::Result<()> {
        let model =
            MarkdownRenderModel::parse_with_language(text, self.content_width(), self.language);
        if self.styled && !self.plain {
            let mut body = model.styled_lines();
            if let Some(footer) = footer {
                body.extend(
                    MarkdownRenderModel::parse_with_language(
                        footer,
                        self.content_width(),
                        self.language,
                    )
                    .styled_lines(),
                );
            }
            return self.write_styled_block(
                output,
                self.i18n().t(crate::MessageId::AgentResponseTitle),
                body,
            );
        }

        let body = if self.plain {
            model.plain_text_lines()
        } else {
            model.rich_text_lines()
        };
        self.write_block(
            output,
            self.i18n().t(crate::MessageId::AgentResponseTitle),
            body,
            footer,
        )
    }

    pub fn write_banner<W: Write>(
        &self,
        output: &mut W,
        title: &str,
        body: Vec<String>,
        footer: Option<&str>,
    ) -> io::Result<()> {
        self.write_block(output, title, body, footer)
    }

    pub fn write_notice_panel<W: Write>(
        &self,
        output: &mut W,
        model: NoticePanelModel<'_>,
    ) -> io::Result<()> {
        let lines = model.body.into_iter().map(Line::from).collect();
        self.write_block(
            output,
            model.title,
            self.render_lines(lines, self.content_width()),
            model.footer,
        )
    }

    pub fn markdown_text_lines(&self, text: &str) -> Vec<String> {
        let model =
            MarkdownRenderModel::parse_with_language(text, self.content_width(), self.language);
        if self.plain {
            model.plain_text_lines()
        } else {
            model.rich_text_lines()
        }
    }

    fn render_lines(&self, lines: Vec<Line<'static>>, width: usize) -> Vec<String> {
        let mut rendered = lines
            .into_iter()
            .flat_map(|line| wrap_plain_line(&strip_ansi_escape(&line_to_string(&line)), width))
            .collect::<Vec<_>>();
        rendered = compact_rendered_lines(rendered);
        while rendered.last().is_some_and(|line| line.trim().is_empty()) {
            rendered.pop();
        }
        rendered
    }

    fn write_block<W: Write>(
        &self,
        output: &mut W,
        title: &str,
        body: Vec<String>,
        footer: Option<&str>,
    ) -> io::Result<()> {
        if self.plain {
            writeln!(output, "{title}:")?;
            for line in body {
                if line.trim().is_empty() {
                    writeln!(output)?;
                } else {
                    writeln!(output, "  {line}")?;
                }
            }
            if let Some(footer) = footer {
                for line in
                    self.render_lines(vec![Line::from(footer.to_string())], self.content_width())
                {
                    writeln!(output, "  {line}")?;
                }
            }
            return Ok(());
        }

        let mut lines = body;
        if let Some(footer) = footer {
            lines.extend(
                self.render_lines(vec![Line::from(footer.to_string())], self.content_width()),
            );
        }
        let rendered_lines = self.rich_block_lines(title, lines);
        for line in rendered_lines {
            writeln!(output, "{line}")?;
        }
        Ok(())
    }

    fn write_styled_block<W: Write>(
        &self,
        output: &mut W,
        title: &str,
        body: Vec<Line<'static>>,
    ) -> io::Result<()> {
        let rendered_lines = self.rich_styled_block_lines(title, body);
        for line in rendered_lines {
            writeln!(output, "{line}")?;
        }
        Ok(())
    }

    fn rich_block_lines(&self, title: &str, body: Vec<String>) -> Vec<String> {
        let width = self.panel_standard_width();
        let inner_width = width.saturating_sub(4).max(1) as usize;
        let content_height = body
            .iter()
            .map(|line| display_width(line).max(1).div_ceil(inner_width).max(1))
            .sum::<usize>()
            .max(1);
        let height = content_height.saturating_add(2).min(200) as u16;
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(1))
            .title(Line::from(Span::styled(
                format!(" {title} "),
                ratatui::style::Style::default().add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        block.render(area, &mut buffer);

        let text = if body.is_empty() {
            Text::from(Line::from(""))
        } else {
            Text::from(body.into_iter().map(Line::from).collect::<Vec<_>>())
        };
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .render(inner, &mut buffer);

        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn rich_styled_block_lines(&self, title: &str, body: Vec<Line<'static>>) -> Vec<String> {
        let width = self.panel_standard_width();
        let inner_width = width.saturating_sub(4).max(1) as usize;
        let content_height = body
            .iter()
            .map(line_to_string)
            .map(|line| display_width(&line).max(1).div_ceil(inner_width).max(1))
            .sum::<usize>()
            .max(1);
        let height = content_height.saturating_add(2).min(200) as u16;
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(1))
            .title(Line::from(Span::styled(
                format!(" {title} "),
                ratatui::style::Style::default().add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        block.render(area, &mut buffer);

        let text = if body.is_empty() {
            Text::from(Line::from(""))
        } else {
            Text::from(body)
        };
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .render(inner, &mut buffer);

        buffer_to_styled_lines(&buffer, area)
    }

    fn panel_standard_width(&self) -> u16 {
        self.width.clamp(MIN_WIDTH, MAX_WIDTH)
    }

    fn content_width(&self) -> usize {
        let margin = if self.plain { 2 } else { 4 };
        self.width
            .clamp(MIN_WIDTH, MAX_WIDTH)
            .saturating_sub(margin)
            .max(20) as usize
    }

    fn supports_status_animation(&self) -> bool {
        if self.plain {
            return false;
        }

        match std::env::var("COSH_SHELL_ANIMATION") {
            Ok(value) if value.eq_ignore_ascii_case("always") => true,
            Ok(value) if value.eq_ignore_ascii_case("never") => false,
            _ => std::io::stdout().is_terminal(),
        }
    }
}

fn buffer_to_lines(buffer: &Buffer, area: Rect) -> Vec<String> {
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut skip = 0usize;
            for x in 0..area.width {
                let symbol = buffer[(x, y)].symbol();
                if skip == 0 {
                    line.push_str(symbol);
                }
                skip = symbol_display_width(symbol).saturating_sub(1);
            }
            line.trim_end().to_string()
        })
        .collect()
}

fn buffer_to_styled_lines(buffer: &Buffer, area: Rect) -> Vec<String> {
    (0..area.height)
        .map(|y| styled_buffer_row(buffer, area, y))
        .collect()
}

fn styled_buffer_row(buffer: &Buffer, area: Rect, y: u16) -> String {
    let Some(last_x) = last_non_blank_cell(buffer, area, y) else {
        return String::new();
    };

    let mut line = String::new();
    let mut skip = 0usize;
    let mut current_style = RenderCellStyle::default();
    let mut used_style = false;
    for x in 0..=last_x {
        let cell = &buffer[(x, y)];
        let style = RenderCellStyle {
            fg: cell.fg,
            bg: cell.bg,
            modifier: cell.modifier,
        };
        if skip == 0 {
            if style != current_style {
                push_ansi_style(&mut line, style);
                current_style = style;
                used_style = true;
            }
            line.push_str(cell.symbol());
        }
        skip = symbol_display_width(cell.symbol()).saturating_sub(1);
    }
    if used_style {
        line.push_str("\x1b[0m");
    }
    line
}

fn last_non_blank_cell(buffer: &Buffer, area: Rect, y: u16) -> Option<u16> {
    (0..area.width).rev().find(|x| {
        let s = buffer[(*x, y)].symbol();
        !s.is_empty() && s != " "
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RenderCellStyle {
    fg: Color,
    bg: Color,
    modifier: Modifier,
}

fn push_ansi_style(line: &mut String, style: RenderCellStyle) {
    let mut codes = vec!["0".to_string()];
    if style.modifier.contains(Modifier::BOLD) {
        codes.push("1".to_string());
    }
    if style.modifier.contains(Modifier::REVERSED) {
        codes.push("7".to_string());
    }
    if let Some(code) = ansi_color_code(style.fg, false) {
        codes.push(code);
    }
    if let Some(code) = ansi_color_code(style.bg, true) {
        codes.push(code);
    }
    line.push_str("\x1b[");
    line.push_str(&codes.join(";"));
    line.push('m');
}

fn ansi_color_code(color: Color, background: bool) -> Option<String> {
    let base = if background { 40 } else { 30 };
    let bright_base = if background { 100 } else { 90 };
    match color {
        Color::Reset => None,
        Color::Black => Some(base.to_string()),
        Color::Red => Some((base + 1).to_string()),
        Color::Green => Some((base + 2).to_string()),
        Color::Yellow => Some((base + 3).to_string()),
        Color::Blue => Some((base + 4).to_string()),
        Color::Magenta => Some((base + 5).to_string()),
        Color::Cyan => Some((base + 6).to_string()),
        Color::Gray => Some((base + 7).to_string()),
        Color::DarkGray => Some(bright_base.to_string()),
        Color::LightRed => Some((bright_base + 1).to_string()),
        Color::LightGreen => Some((bright_base + 2).to_string()),
        Color::LightYellow => Some((bright_base + 3).to_string()),
        Color::LightBlue => Some((bright_base + 4).to_string()),
        Color::LightMagenta => Some((bright_base + 5).to_string()),
        Color::LightCyan => Some((bright_base + 6).to_string()),
        Color::White => Some((bright_base + 7).to_string()),
        Color::Indexed(index) => Some(format!("{};5;{}", if background { 48 } else { 38 }, index)),
        Color::Rgb(red, green, blue) => Some(format!(
            "{};2;{};{};{}",
            if background { 48 } else { 38 },
            red,
            green,
            blue
        )),
    }
}

fn symbol_display_width(symbol: &str) -> usize {
    if symbol.is_empty() {
        return 0;
    }
    display_width(symbol).max(1)
}

fn lines_from_governed_events(
    governed_events: &[GovernedEvent],
    i18n: &crate::I18n,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for event in governed_events {
        for (idx, line) in display_lines_for_event(event, i18n).into_iter().enumerate() {
            let prefix = if idx == 0 { "" } else { "  " };
            lines.push(Line::from(vec![
                Span::from(prefix.to_string()),
                Span::from(line),
            ]));
        }
    }
    lines
}

fn display_lines_for_event(event: &GovernedEvent, i18n: &crate::I18n) -> Vec<String> {
    match &event.event {
        AgentEvent::Recommendation {
            summary, commands, ..
        } => {
            let mut lines = vec![summary.clone()];
            if !commands.is_empty() {
                lines.push(
                    i18n.t(crate::MessageId::AgentRecommendedCommandsLabel)
                        .to_string(),
                );
                lines.extend(commands.iter().map(|command| format!("  - {command}")));
            }
            lines
        }
        AgentEvent::AgentCancelled { reason, .. } => vec![
            i18n.t(crate::MessageId::FailedAnalysisCancelledTitle)
                .to_string(),
            format!(
                "{} {}",
                i18n.t(crate::MessageId::AgentCancelledReasonLabel),
                agent_cancelled_reason_label(reason, i18n)
            ),
        ],
        _ => event
            .display_text
            .lines()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    }
}

fn agent_cancelled_reason_label(reason: &str, i18n: &crate::I18n) -> String {
    if reason == "user requested cancellation" {
        return i18n
            .t(crate::MessageId::AgentCancelledUserRequestedReason)
            .to_string();
    }
    reason.to_string()
}

fn plain_output_requested() -> bool {
    std::env::var("COSH_SHELL_RENDER")
        .map(|mode| mode.eq_ignore_ascii_case("plain") || mode.eq_ignore_ascii_case("text"))
        .unwrap_or(false)
        || std::env::var("TERM")
            .map(|term| term.eq_ignore_ascii_case("dumb"))
            .unwrap_or(false)
}

fn configured_terminal_width() -> Option<u16> {
    std::env::var("COSH_SHELL_WIDTH")
        .ok()
        .and_then(|width| width.parse::<u16>().ok())
        .map(|width| width.clamp(MIN_WIDTH, 200))
}

#[cfg(test)]
mod tests;
