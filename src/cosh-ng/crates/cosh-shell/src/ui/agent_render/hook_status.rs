//! `/hooks` status panel: shell-hook stats plus agent hooks grouped by
//! event, sharing the reference-panel visual contract with `/help`.

use std::io::{self, Write};

use ratatui::text::{Line, Span};

use super::reference_style::{
    reference_body_style, reference_emphasis_style, reference_group_style, reference_muted_style,
    reference_section_style,
};
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
    pub(crate) footer: String,
}

/// Agent hooks section content: grouped hooks, or a status message
/// (backend unavailable, registry error, empty registry).
#[derive(Debug)]
pub(crate) enum AgentHooksView {
    Groups(Vec<HookEventGroup>),
    Message(String),
}

#[derive(Debug)]
pub(crate) struct HookEventGroup {
    pub(crate) event: String,
    pub(crate) hooks: Vec<HookEntryView>,
}

#[derive(Debug)]
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
            let body = plain_hook_status_lines(&model);
            return self.write_block(output, model.title, body, Some(&model.footer));
        }
        let title = model.title;
        let body = styled_hook_status_lines(&model);
        self.write_styled_block(output, title, body)
    }
}

fn styled_hook_status_lines(model: &HookStatusPanelModel<'_>) -> Vec<Line<'static>> {
    let section = reference_section_style();
    let group = reference_group_style();
    let emphasis = reference_emphasis_style();
    let muted = reference_muted_style();
    let body_style = reference_body_style();

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        model.shell_label.to_string(),
        section,
    )));
    for shell_line in &model.shell_lines {
        lines.push(Line::from(Span::styled(
            format!("  {}", shell_line.trim_start()),
            body_style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        model.agent_label.to_string(),
        section,
    )));
    match &model.agent {
        AgentHooksView::Message(message) => {
            lines.push(Line::from(Span::styled(
                format!("  {}", message.trim_start()),
                body_style,
            )));
        }
        AgentHooksView::Groups(groups) => {
            for (index, event_group) in groups.iter().enumerate() {
                if index > 0 {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    format!("  {}", event_group.event),
                    group,
                )));
                for hook in &event_group.hooks {
                    if hook.disabled {
                        lines.push(Line::from(Span::styled(
                            format!("    ○ {} (ext: {}) [disabled]", hook.name, hook.extension),
                            muted,
                        )));
                    } else {
                        lines.push(Line::from(vec![
                            Span::raw("    ".to_string()),
                            Span::styled("• ".to_string(), emphasis),
                            Span::raw(hook.name.clone()),
                            Span::styled(format!(" (ext: {})", hook.extension), muted),
                        ]));
                    }
                }
            }
        }
    }
    // Spacer before the footer is owned by the styled layout; the plain
    // backend delegates footer spacing to write_block instead.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(model.footer.clone(), body_style)));
    lines
}

fn plain_hook_status_lines(model: &HookStatusPanelModel<'_>) -> Vec<String> {
    let mut body = Vec::new();
    body.push(format!("── {} ──", model.shell_label));
    body.extend(model.shell_lines.iter().cloned());
    body.push(format!("── {} ──", model.agent_label));
    match &model.agent {
        AgentHooksView::Message(message) => body.push(format!("  {}", message.trim_start())),
        AgentHooksView::Groups(groups) => {
            for event_group in groups {
                body.push(format!("  {}", event_group.event));
                for hook in &event_group.hooks {
                    if hook.disabled {
                        body.push(format!(
                            "    ○ {} (ext: {}) [disabled]",
                            hook.name, hook.extension
                        ));
                    } else {
                        body.push(format!("    • {} (ext: {})", hook.name, hook.extension));
                    }
                }
            }
        }
    }
    body
}
