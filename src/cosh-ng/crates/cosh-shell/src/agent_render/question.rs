use std::io::{self, Write};

use crate::{
    question_choice_count as shared_question_choice_count, question_custom_answer_index,
    types::QuestionSelectionMode,
};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{block::Padding, Block, Paragraph, Widget, Wrap},
};

use super::{
    buffer_to_lines, buffer_to_styled_lines, display_width, wrap_plain_line, RatatuiInlineRenderer,
    MIN_WIDTH, QUESTION_MAX_WIDTH,
};

#[derive(Debug, Clone)]
pub struct QuestionPanelModel<'a> {
    pub id: &'a str,
    pub question: &'a str,
    pub options: &'a [String],
    pub selected_option: usize,
    pub selected_options: &'a [usize],
    pub custom_answer: &'a str,
    pub allow_free_text: bool,
    pub selection_mode: QuestionSelectionMode,
}

#[derive(Debug, Clone)]
pub struct QuestionAnswerPanelModel<'a> {
    pub id: &'a str,
    pub question: &'a str,
    pub answer: &'a str,
    pub message: &'a str,
}

impl RatatuiInlineRenderer {
    pub fn write_question_panel<W: Write>(
        &self,
        output: &mut W,
        model: QuestionPanelModel<'_>,
    ) -> io::Result<usize> {
        let lines = self.question_panel_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn question_panel_lines(&self, model: QuestionPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_question_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, QUESTION_MAX_WIDTH);
        let height = question_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_question_panel(model, area, &mut buffer);
        buffer_to_lines(&buffer, area)
    }

    fn question_panel_write_lines(&self, model: QuestionPanelModel<'_>) -> Vec<String> {
        if self.plain {
            return self.plain_question_panel_lines(model);
        }

        let width = self.width.clamp(MIN_WIDTH, QUESTION_MAX_WIDTH);
        let height = question_panel_height(&model, width);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_question_panel(model, area, &mut buffer);
        if self.styled {
            buffer_to_styled_lines(&buffer, area)
        } else {
            buffer_to_lines(&buffer, area)
        }
    }

    fn plain_question_panel_lines(&self, model: QuestionPanelModel<'_>) -> Vec<String> {
        let width = self.width.clamp(MIN_WIDTH, QUESTION_MAX_WIDTH);
        let content_width = question_content_width(width);
        let mut lines = vec!["Agent question".to_string()];
        lines.extend(wrap_plain_line(model.question, content_width));
        if !model.options.is_empty() {
            let selected = selected_option(&model);
            lines.push(question_option_heading(&model).to_string());
            lines.extend(
                model.options.iter().enumerate().flat_map(|(idx, option)| {
                    plain_option_lines(&model, idx, option, content_width)
                }),
            );
            if let Some(idx) =
                question_custom_answer_index(model.options.len(), model.allow_free_text)
            {
                let marker = if selected == idx { "> " } else { "  " };
                let prefix = format!("{marker}[{}] ", idx + 1);
                lines.extend(wrap_option_text(
                    &prefix,
                    &custom_option_label(model.custom_answer),
                    content_width,
                ));
            }
            lines.extend(wrap_plain_line(
                &instruction_text(&model, selected),
                content_width,
            ));
        }
        if model.options.is_empty() {
            lines.extend(wrap_plain_line(
                &instruction_text(&model, selected_option(&model)),
                content_width,
            ));
        }
        lines
    }

    pub fn write_question_answer_panel<W: Write>(
        &self,
        output: &mut W,
        model: QuestionAnswerPanelModel<'_>,
    ) -> io::Result<usize> {
        let lines = self.question_answer_panel_write_lines(model);
        for line in &lines {
            writeln!(output, "{line}")?;
        }
        Ok(lines.len())
    }

    pub fn question_answer_panel_lines(&self, model: QuestionAnswerPanelModel<'_>) -> Vec<String> {
        let width = self.width.clamp(MIN_WIDTH, QUESTION_MAX_WIDTH);
        self.question_answer_lines(model, width, false)
    }

    fn question_answer_panel_write_lines(
        &self,
        model: QuestionAnswerPanelModel<'_>,
    ) -> Vec<String> {
        let width = self.width.clamp(MIN_WIDTH, QUESTION_MAX_WIDTH);
        self.question_answer_lines(model, width, self.styled)
    }

    fn question_answer_lines(
        &self,
        model: QuestionAnswerPanelModel<'_>,
        width: u16,
        styled: bool,
    ) -> Vec<String> {
        let content_width = question_content_width(width);
        let lines = wrapped_question_label_rows("Answer", model.answer, content_width);
        if !styled {
            return lines;
        }

        let area = Rect::new(0, 0, width, lines.len() as u16);
        let mut buffer = Buffer::empty(area);
        let styled_lines = lines
            .into_iter()
            .map(|line| {
                Line::from(Span::styled(
                    line,
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect::<Vec<_>>();
        Paragraph::new(Text::from(styled_lines)).render(area, &mut buffer);
        buffer_to_styled_lines(&buffer, area)
    }
}

fn question_panel_height(model: &QuestionPanelModel<'_>, width: u16) -> u16 {
    let content_width = question_content_width(width);
    question_rows(model, content_width)
        + option_rows(model, content_width)
        + instruction_rows(model, content_width)
        + 2
}

fn render_question_panel(model: QuestionPanelModel<'_>, area: Rect, buffer: &mut Buffer) {
    let selected_option = selected_option(&model);
    let block = Block::bordered()
        .padding(Padding::horizontal(1))
        .title(Line::from(Span::styled(
            " Agent question ",
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    block.render(area, buffer);

    let content_width = inner.width as usize;
    let question_rows = question_rows(&model, content_width);
    let option_rows = option_rows(&model, content_width);
    let instruction_rows = instruction_rows(&model, content_width);
    let chunks = Layout::vertical(vec![
        Constraint::Length(question_rows),
        Constraint::Length(option_rows),
        Constraint::Length(instruction_rows),
    ])
    .split(inner);

    Paragraph::new(model.question.to_string())
        .wrap(Wrap { trim: true })
        .render(chunks[0], buffer);

    let mut option_lines = vec![Line::from(Span::styled(
        question_option_heading(&model).to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if !model.options.is_empty() {
        option_lines.extend(rendered_option_lines(
            &model,
            selected_option,
            content_width,
        ));
        if let Some(idx) = question_custom_answer_index(model.options.len(), model.allow_free_text)
        {
            let selected = idx == selected_option;
            option_lines.extend(render_custom_option_lines(
                idx,
                selected,
                model.custom_answer,
                content_width,
            ));
        }
    }
    if option_rows > 0 {
        Paragraph::new(Text::from(option_lines))
            .wrap(Wrap { trim: false })
            .render(chunks[1], buffer);
    }

    let instruction = instruction_text(&model, selected_option);
    Paragraph::new(Line::from(vec![
        Span::styled("Keys: ", Style::default().fg(Color::DarkGray)),
        Span::raw(instruction),
    ]))
    .wrap(Wrap { trim: true })
    .render(chunks[2], buffer);
}

fn selected_option(model: &QuestionPanelModel<'_>) -> usize {
    model
        .selected_option
        .min(question_choice_count(model).saturating_sub(1))
}

fn question_content_width(width: u16) -> usize {
    width.saturating_sub(4).max(20) as usize
}

fn question_rows(model: &QuestionPanelModel<'_>, width: usize) -> u16 {
    wrapped_row_count(model.question, width)
}

fn option_rows(model: &QuestionPanelModel<'_>, width: usize) -> u16 {
    if model.options.is_empty() {
        return 0;
    }
    let option_count = model
        .options
        .iter()
        .enumerate()
        .map(|(idx, option)| wrapped_row_count(&option_line_text(model, idx, option), width))
        .sum::<u16>()
        + question_custom_answer_index(model.options.len(), model.allow_free_text)
            .map(|idx| wrapped_row_count(&custom_option_text(idx, model.custom_answer), width))
            .unwrap_or(0);
    1 + option_count
}

fn instruction_rows(model: &QuestionPanelModel<'_>, width: usize) -> u16 {
    wrapped_row_count(&instruction_text(model, selected_option(model)), width)
}

fn instruction_text(model: &QuestionPanelModel<'_>, selected_option: usize) -> String {
    if !model.options.is_empty() {
        let custom_selected =
            question_custom_answer_index(model.options.len(), model.allow_free_text)
                .is_some_and(|idx| selected_option >= idx);
        if model.selection_mode == QuestionSelectionMode::Multiple {
            if custom_selected {
                "Left/Right move | type answer | Enter send".to_string()
            } else {
                "Left/Right move | Space toggle | Enter send".to_string()
            }
        } else if custom_selected {
            "Left/Right move | type answer | Enter send".to_string()
        } else {
            "Left/Right move | Enter send".to_string()
        }
    } else if model.allow_free_text {
        "Type answer | Enter send".to_string()
    } else {
        "No selectable answer is available.".to_string()
    }
}

fn wrapped_row_count(text: &str, width: usize) -> u16 {
    wrap_plain_line(text, width).len().max(1) as u16
}

fn question_choice_count(model: &QuestionPanelModel<'_>) -> usize {
    shared_question_choice_count(model.options.len(), model.allow_free_text)
}

fn plain_option_lines(
    model: &QuestionPanelModel<'_>,
    idx: usize,
    option: &str,
    width: usize,
) -> Vec<String> {
    let marker = if selected_option(model) == idx {
        "> "
    } else {
        "  "
    };
    let prefix = if model.selection_mode == QuestionSelectionMode::Multiple {
        let checkbox = if option_is_checked(model, idx) {
            "x"
        } else {
            " "
        };
        format!("{marker}[{checkbox}] [{}] ", idx + 1)
    } else {
        format!("{marker}[{}] ", idx + 1)
    };
    wrap_option_text(&prefix, option, width)
}

fn option_line_text(model: &QuestionPanelModel<'_>, idx: usize, option: &str) -> String {
    let marker = if selected_option(model) == idx {
        "> "
    } else {
        "  "
    };
    if model.selection_mode == QuestionSelectionMode::Multiple {
        let checkbox = if option_is_checked(model, idx) {
            "x"
        } else {
            " "
        };
        format!("{marker}[{checkbox}] [{}] {}", idx + 1, option)
    } else {
        format!("{marker}[{}] {}", idx + 1, option)
    }
}

fn question_option_heading(model: &QuestionPanelModel<'_>) -> &'static str {
    if model.options.is_empty() {
        "Answer:"
    } else if model.selection_mode == QuestionSelectionMode::Multiple {
        "Select one or more:"
    } else {
        "Select one:"
    }
}

fn rendered_option_lines(
    model: &QuestionPanelModel<'_>,
    selected_option: usize,
    width: usize,
) -> Vec<Line<'static>> {
    model
        .options
        .iter()
        .enumerate()
        .flat_map(|(idx, option)| {
            let selected = idx == selected_option;
            let checked = option_is_checked(model, idx);
            let marker = if selected { ">" } else { " " };
            let prefix = if model.selection_mode == QuestionSelectionMode::Multiple {
                let checkbox = if checked { "x" } else { " " };
                format!("{marker} [{checkbox}] [{}] ", idx + 1)
            } else {
                format!("{marker} [{}] ", idx + 1)
            };
            render_wrapped_option(&prefix, option, option_marker_style(selected), width)
        })
        .collect()
}

fn render_custom_option_lines(
    idx: usize,
    selected: bool,
    custom_answer: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let marker = if selected { ">" } else { " " };
    let prefix = format!("{marker} [{}] ", idx + 1);
    render_wrapped_option(
        &prefix,
        &custom_option_label(custom_answer),
        option_marker_style(selected),
        width,
    )
}

fn render_wrapped_option(
    prefix: &str,
    text: &str,
    prefix_style: Style,
    width: usize,
) -> Vec<Line<'static>> {
    wrap_option_text(prefix, text, width)
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            if idx == 0 {
                let prefix_len = prefix.len().min(line.len());
                let (prefix, rest) = line.split_at(prefix_len);
                Line::from(vec![
                    Span::styled(prefix.to_string(), prefix_style),
                    Span::raw(rest.to_string()),
                ])
            } else {
                Line::from(Span::raw(line))
            }
        })
        .collect()
}

fn custom_option_text(idx: usize, custom_answer: &str) -> String {
    format!("  [{}] {}", idx + 1, custom_option_label(custom_answer))
}

fn custom_option_label(custom_answer: &str) -> String {
    if custom_answer.is_empty() {
        "Other...".to_string()
    } else {
        format!("Other: {custom_answer}")
    }
}

fn wrapped_question_label_rows(label: &str, text: &str, width: usize) -> Vec<String> {
    let prefix = format!("{label}: ");
    let continuation = " ".repeat(display_width(&prefix));
    let content_width = width.saturating_sub(display_width(&prefix)).max(1);
    wrap_plain_line(text, content_width)
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            if idx == 0 {
                format!("{prefix}{line}")
            } else {
                format!("{continuation}{line}")
            }
        })
        .collect()
}

fn wrap_option_text(prefix: &str, text: &str, width: usize) -> Vec<String> {
    let width = width.max(display_width(prefix) + 1);
    let continuation = " ".repeat(display_width(prefix));
    let mut lines = Vec::new();
    let mut current = prefix.to_string();
    let mut current_width = display_width(prefix);

    for token in split_option_wrap_tokens(text) {
        let token_width = display_width(&token);
        if token_width > 0 && current_width + token_width > width && current.trim() != prefix.trim()
        {
            lines.push(current.trim_end().to_string());
            current = continuation.clone();
        }
        current.push_str(if current.trim() == prefix.trim() {
            token.trim_start()
        } else {
            &token
        });
        current_width = display_width(&current);
    }

    if !current.trim().is_empty() || lines.is_empty() {
        lines.push(current.trim_end().to_string());
    }
    lines
}

fn split_option_wrap_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut last_was_space = None;

    for ch in text.chars() {
        let is_space = ch.is_whitespace();
        if last_was_space.is_some_and(|was_space| was_space != is_space) && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        current.push(ch);
        last_was_space = Some(is_space);
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn option_is_checked(model: &QuestionPanelModel<'_>, idx: usize) -> bool {
    model.selection_mode == QuestionSelectionMode::Multiple && model.selected_options.contains(&idx)
}

fn option_marker_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    }
}
