//! `/hooks` status panel: shell-hook stats plus agent hooks grouped by
//! event, sharing the reference-panel visual contract with `/help`.

use std::io::{self, Write};

use ratatui::text::{Line, Span};

use super::reference_style::{
    reference_body_style, reference_emphasis_style, reference_group_style, reference_muted_style,
    reference_section_style,
};
use super::wrap::{strip_ansi_escape, wrap_plain_line, wrap_plain_line_with_prefix};
use super::RatatuiInlineRenderer;

#[derive(Debug)]
pub(crate) struct HookStatusPanelModel<'a> {
    pub(crate) title: &'a str,
    pub(crate) shell_label: &'a str,
    /// Shell-hook stat lines as logical content without indentation
    /// prefixes; each backend applies its own indentation (styled indents
    /// under the section header, plain keeps the legacy flush layout).
    pub(crate) shell_lines: Vec<String>,
    pub(crate) agent_label: &'a str,
    pub(crate) agent: AgentHooksView,
    /// Localized template for the truncation marker on the styled backend;
    /// `{count}` is replaced with the number of hooks not shown. The plain
    /// backend never truncates.
    pub(crate) omitted_template: String,
    pub(crate) footer: String,
}

/// Agent hooks section content: grouped hooks, or a status message
/// (backend unavailable, registry error, empty registry).
#[derive(Debug)]
pub(crate) enum AgentHooksView {
    Groups(Vec<HookEventGroup>),
    Message(String),
}

#[derive(Debug, Clone)]
pub(crate) struct HookEventGroup {
    pub(crate) event: String,
    pub(crate) hooks: Vec<HookEntryView>,
}

#[derive(Debug, Clone)]
pub(crate) struct HookEntryView {
    pub(crate) name: String,
    pub(crate) extension: String,
    pub(crate) disabled: bool,
}

impl RatatuiInlineRenderer {
    pub(crate) fn write_hook_status_panel<W: Write>(
        &self,
        output: &mut W,
        model: HookStatusPanelModel<'_>,
    ) -> io::Result<()> {
        if self.plain {
            let width = self.content_width();
            let body = plain_hook_status_lines(&model, width);
            return self.write_block(output, model.title, body, Some(&model.footer));
        }
        let inner = usize::from(self.panel_standard_width().saturating_sub(4)).max(1);
        let title = model.title;
        let body = styled_hook_status_lines(&model, inner);
        self.write_styled_block(output, title, body)
    }
}

/// Removes ANSI escape sequences and any remaining control characters from
/// registry- or error-provided text before it reaches a terminal. The legacy
/// notice-panel path stripped escapes in `render_lines`; this panel writes
/// spans directly, so sanitizing is this module's responsibility for both
/// backends.
fn sanitize(text: &str) -> String {
    strip_ansi_escape(text)
        .chars()
        .filter(|ch| !ch.is_control())
        .collect()
}

const ENTRY_INDENT: &str = "    ";
const ENTRY_HANG: &str = "      ";

/// The rich block renderer caps a panel at 200 buffer rows (borders
/// included). Grouped agent hooks can exceed that on large registries, and
/// `Paragraph` would silently drop the overflow — including the footer. Keep
/// the body within this budget and surface an explicit omission marker
/// instead; the reserve covers the marker, the pre-footer spacer, and the
/// footer itself.
const STYLED_BODY_LINE_BUDGET: usize = 195;

fn styled_hook_status_lines(model: &HookStatusPanelModel<'_>, inner: usize) -> Vec<Line<'static>> {
    let section = reference_section_style();
    let group = reference_group_style();
    let emphasis = reference_emphasis_style();
    let muted = reference_muted_style();
    let body_style = reference_body_style();

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        sanitize(model.shell_label),
        section,
    )));
    for shell_line in &model.shell_lines {
        for segment in
            wrap_plain_line_with_prefix(&sanitize(shell_line.trim_start()), "  ", "  ", inner)
        {
            lines.push(Line::from(Span::styled(segment, body_style)));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        sanitize(model.agent_label),
        section,
    )));
    match &model.agent {
        AgentHooksView::Message(message) => {
            for segment in
                wrap_plain_line_with_prefix(&sanitize(message.trim_start()), "  ", "  ", inner)
            {
                lines.push(Line::from(Span::styled(segment, body_style)));
            }
        }
        AgentHooksView::Groups(groups) => {
            let mut omitted = 0usize;
            for (index, event_group) in groups.iter().enumerate() {
                // Stop emitting once the line budget is spent; keep counting
                // the hooks that will not be shown.
                if lines.len() >= STYLED_BODY_LINE_BUDGET {
                    omitted += event_group.hooks.len();
                    continue;
                }
                if index > 0 {
                    lines.push(Line::from(""));
                }
                for segment in
                    wrap_plain_line_with_prefix(&sanitize(&event_group.event), "  ", "  ", inner)
                {
                    lines.push(Line::from(Span::styled(segment, group)));
                }
                for hook in &event_group.hooks {
                    if lines.len() >= STYLED_BODY_LINE_BUDGET {
                        omitted += 1;
                        continue;
                    }
                    let name = sanitize(&hook.name);
                    let extension = sanitize(&hook.extension);
                    if hook.disabled {
                        for segment in wrap_plain_line_with_prefix(
                            &format!("○ {name} (ext: {extension}) [disabled]"),
                            ENTRY_INDENT,
                            ENTRY_HANG,
                            inner,
                        ) {
                            lines.push(Line::from(Span::styled(segment, muted)));
                        }
                    } else {
                        let wrapped = wrap_plain_line_with_prefix(
                            &format!("• {name} (ext: {extension})"),
                            ENTRY_INDENT,
                            ENTRY_HANG,
                            inner,
                        );
                        for (segment_index, segment) in wrapped.iter().enumerate() {
                            if segment_index == 0 {
                                // First segment: "    " + "• " + name [+ metadata tail].
                                let rest = segment
                                    .strip_prefix(ENTRY_INDENT)
                                    .and_then(|rest| rest.strip_prefix("• "));
                                match rest {
                                    Some(rest) => {
                                        let (head, tail) = match rest.find(" (ext:") {
                                            Some(split) => rest.split_at(split),
                                            None => (rest, ""),
                                        };
                                        let mut spans = vec![
                                            Span::raw(ENTRY_INDENT.to_string()),
                                            Span::styled("• ".to_string(), emphasis),
                                            Span::raw(head.to_string()),
                                        ];
                                        if !tail.is_empty() {
                                            spans.push(Span::styled(tail.to_string(), muted));
                                        }
                                        lines.push(Line::from(spans));
                                    }
                                    None => lines.push(Line::from(Span::raw(segment.clone()))),
                                }
                            } else {
                                // Continuation segments carry metadata overflow.
                                lines.push(Line::from(Span::styled(segment.clone(), muted)));
                            }
                        }
                    }
                }
            }
            if omitted > 0 {
                let marker = model
                    .omitted_template
                    .replace("{count}", &omitted.to_string());
                for segment in wrap_plain_line_with_prefix(&sanitize(&marker), "  ", "  ", inner) {
                    lines.push(Line::from(Span::styled(segment, muted)));
                }
            }
        }
    }
    // Spacer before the footer is owned by the styled layout; the plain
    // backend delegates footer spacing to write_block instead.
    lines.push(Line::from(""));
    for segment in wrap_plain_line(&sanitize(&model.footer), inner) {
        lines.push(Line::from(Span::styled(segment, body_style)));
    }
    lines
}

fn plain_hook_status_lines(model: &HookStatusPanelModel<'_>, width: usize) -> Vec<String> {
    let mut body = Vec::new();
    body.push(format!("── {} ──", sanitize(model.shell_label)));
    for shell_line in &model.shell_lines {
        body.extend(wrap_plain_line(&sanitize(shell_line), width));
    }
    body.push(format!("── {} ──", sanitize(model.agent_label)));
    match &model.agent {
        AgentHooksView::Message(message) => body.extend(wrap_plain_line_with_prefix(
            &sanitize(message.trim_start()),
            "  ",
            "  ",
            width,
        )),
        AgentHooksView::Groups(groups) => {
            for event_group in groups {
                body.extend(wrap_plain_line_with_prefix(
                    &sanitize(&event_group.event),
                    "  ",
                    "  ",
                    width,
                ));
                for hook in &event_group.hooks {
                    let name = sanitize(&hook.name);
                    let extension = sanitize(&hook.extension);
                    let entry = if hook.disabled {
                        format!("○ {name} (ext: {extension}) [disabled]")
                    } else {
                        format!("• {name} (ext: {extension})")
                    };
                    body.extend(wrap_plain_line_with_prefix(
                        &entry,
                        ENTRY_INDENT,
                        ENTRY_HANG,
                        width,
                    ));
                }
            }
        }
    }
    body
}
