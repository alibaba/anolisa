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

/// The rich block renderer caps a panel at 200 buffer rows, including borders.
const STYLED_BODY_LINE_LIMIT: usize = 198;

fn styled_hook_entry_lines(hook: &HookEntryView, inner: usize) -> Vec<Line<'static>> {
    let emphasis = reference_emphasis_style();
    let muted = reference_muted_style();
    let name = sanitize(&hook.name);
    let extension = sanitize(&hook.extension);
    if hook.disabled {
        return wrap_plain_line_with_prefix(
            &format!("○ {name} (ext: {extension}) [disabled]"),
            ENTRY_INDENT,
            ENTRY_HANG,
            inner,
        )
        .into_iter()
        .map(|segment| Line::from(Span::styled(segment, muted)))
        .collect();
    }

    wrap_plain_line_with_prefix(
        &format!("• {name} (ext: {extension})"),
        ENTRY_INDENT,
        ENTRY_HANG,
        inner,
    )
    .into_iter()
    .enumerate()
    .map(|(segment_index, segment)| {
        if segment_index > 0 {
            return Line::from(Span::styled(segment, muted));
        }

        // Preserve the enabled-entry emphasis while keeping wrapped metadata muted.
        let rest = segment
            .strip_prefix(ENTRY_INDENT)
            .and_then(|rest| rest.strip_prefix("• "));
        let Some(rest) = rest else {
            return Line::from(Span::raw(segment));
        };
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
        Line::from(spans)
    })
    .collect()
}

fn styled_hook_status_lines(model: &HookStatusPanelModel<'_>, inner: usize) -> Vec<Line<'static>> {
    let section = reference_section_style();
    let group = reference_group_style();
    let muted = reference_muted_style();
    let body_style = reference_body_style();
    let footer_lines = wrap_plain_line(&sanitize(&model.footer), inner)
        .into_iter()
        .map(|segment| Line::from(Span::styled(segment, body_style)))
        .collect::<Vec<_>>();

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
            let total_hooks: usize = groups
                .iter()
                .map(|event_group| event_group.hooks.len())
                .sum();
            let marker_reserve = model
                .omitted_template
                .replace("{count}", &total_hooks.to_string());
            let marker_reserve =
                wrap_plain_line_with_prefix(&sanitize(&marker_reserve), "  ", "  ", inner).len();
            let tail_reserve = marker_reserve + 1 + footer_lines.len();
            let mut shown_hooks = 0usize;
            let mut shown_groups = 0usize;

            'groups: for event_group in groups {
                if event_group.hooks.is_empty() {
                    continue;
                }
                let mut group_prefix = Vec::new();
                if shown_groups > 0 {
                    group_prefix.push(Line::from(""));
                }
                group_prefix.extend(
                    wrap_plain_line_with_prefix(&sanitize(&event_group.event), "  ", "  ", inner)
                        .into_iter()
                        .map(|segment| Line::from(Span::styled(segment, group))),
                );
                let mut group_prefix = Some(group_prefix);

                for hook in &event_group.hooks {
                    let hook_lines = styled_hook_entry_lines(hook, inner);
                    let prefix_len = group_prefix.as_ref().map_or(0, Vec::len);
                    let required = lines.len() + prefix_len + hook_lines.len() + tail_reserve;
                    if required > STYLED_BODY_LINE_LIMIT {
                        break 'groups;
                    }
                    if let Some(prefix) = group_prefix.take() {
                        lines.extend(prefix);
                        shown_groups += 1;
                    }
                    lines.extend(hook_lines);
                    shown_hooks += 1;
                }
            }
            let omitted = total_hooks - shown_hooks;
            if omitted > 0 {
                let marker = model
                    .omitted_template
                    .replace("{count}", &omitted.to_string());
                for segment in wrap_plain_line_with_prefix(&sanitize(&marker), "  ", "  ", inner) {
                    lines.push(Line::from(Span::styled(segment, muted)));
                }
            }
            lines.push(Line::from(""));
            lines.extend(footer_lines);
            debug_assert!(lines.len() <= STYLED_BODY_LINE_LIMIT);
            return lines;
        }
    }
    // Spacer before the footer is owned by the styled layout; the plain
    // backend delegates footer spacing to write_block instead.
    lines.push(Line::from(""));
    lines.extend(footer_lines);
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
