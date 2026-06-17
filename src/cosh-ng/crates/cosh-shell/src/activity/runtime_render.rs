use std::io::Write;

use crate::runtime::prelude::*;

use super::runtime::{ActivityKind, RuntimeActivityRow};

pub(crate) fn render_activity_rows<W: Write>(
    state: &InlineState,
    activity_ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    let rows = activity_ids
        .iter()
        .filter_map(|activity_id| {
            state
                .activity
                .rows
                .iter()
                .find(|row| row.id == *activity_id)
        })
        .filter(|row| state.debug || row.status != "loading")
        .filter(|row| should_render_activity_row_with_state(row, state))
        .map(|row| ActivityRowModel {
            id: &row.id,
            kind: row.kind.label(),
            status: &row.status,
            subject: &row.subject,
            summary: &row.summary,
        })
        .collect::<Vec<_>>();

    if rows.is_empty() {
        return Ok(());
    }

    RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_activity_panel(output, ActivityPanelModel { rows })?;
    Ok(())
}

pub(crate) fn render_provider_native_shell_transcript<W: Write>(
    state: &mut InlineState,
    activity_ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    let rows = activity_ids
        .iter()
        .filter_map(|activity_id| {
            state
                .activity
                .rows
                .iter()
                .find(|row| row.id == *activity_id)
                .cloned()
        })
        .collect::<Vec<_>>();

    for row in rows {
        match row.kind {
            ActivityKind::ToolOutput => {
                let tool_id = row.subject.as_str();
                let Some(command) = state
                    .control
                    .provider_tool()
                    .command(tool_id)
                    .map(|command| command.command.clone())
                else {
                    continue;
                };
                if state.control.provider_shell_transcript_output_seen(tool_id) {
                    continue;
                }
                if state
                    .control
                    .claim_provider_shell_transcript_command(tool_id)
                {
                    writeln!(output, "$ {command}")?;
                }
                let text = state
                    .control
                    .provider_tool()
                    .output_text(tool_id)
                    .unwrap_or_default();
                output.write_all(text.as_bytes())?;
                if !text.is_empty() && !text.ends_with('\n') {
                    writeln!(output)?;
                }
                state.control.mark_provider_shell_transcript_output(tool_id);
            }
            ActivityKind::Tool => {
                if matches!(
                    row.status.as_str(),
                    "called" | "requested" | "auto-approved"
                ) {
                    continue;
                }
                let tool_id = row.subject.as_str();
                let Some(command) = state
                    .control
                    .provider_tool()
                    .command(tool_id)
                    .map(|command| command.command.clone())
                else {
                    continue;
                };
                if state.control.provider_shell_transcript_output_seen(tool_id) {
                    continue;
                }
                if state
                    .control
                    .claim_provider_shell_transcript_command(tool_id)
                {
                    writeln!(output, "$ {command}")?;
                }
                if !matches!(
                    row.status.as_str(),
                    "success" | "completed" | "auto-approved"
                ) {
                    writeln!(output, "tool status: {}", row.status)?;
                }
            }
            _ => {}
        }
    }

    output.flush()
}

pub(crate) fn render_activity_details<W: Write>(
    language: Language,
    row: &RuntimeActivityRow,
    debug: bool,
    output: &mut W,
) -> std::io::Result<()> {
    let detail = activity_detail_for_render(row, debug);
    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_activity_details_panel(
            output,
            ActivityDetailsPanelModel {
                id: &row.id,
                run_id: &row.run_id,
                kind: row.kind.label(),
                status: &row.status,
                subject: &row.subject,
                summary: &row.summary,
                detail: &detail,
            },
        )?;
    Ok(())
}

pub(crate) fn render_activity_details_by_id<W: Write>(
    state: &InlineState,
    id: &str,
    output: &mut W,
) -> Option<std::io::Result<()>> {
    state
        .activity
        .rows
        .iter()
        .find(|row| row.id == id)
        .map(|row| render_activity_details(state.language, row, state.debug, output))
}

fn should_render_activity_row(row: &RuntimeActivityRow, approval_mode: CoshApprovalMode) -> bool {
    match row.kind {
        ActivityKind::Skill => row.status == "failed",
        ActivityKind::ShellHandoff => row.status != "completed",
        ActivityKind::ToolOutput => false,
        ActivityKind::Tool => {
            if activity_row_is_question_tool(row) {
                return false;
            }
            let needs_foreground_shell = row
                .detail
                .contains("interactive_hint: may_require_foreground_shell");
            if activity_row_is_shell_tool(row) {
                return needs_foreground_shell;
            }
            if approval_mode != CoshApprovalMode::Recommend
                && activity_row_is_control_permission(row)
                && row.status == "requested"
            {
                return false;
            }
            matches!(row.status.as_str(), "error" | "failed" | "interrupted")
                || needs_foreground_shell
                || matches!(row.status.as_str(), "called" | "requested")
        }
    }
}

fn should_render_activity_row_with_state(row: &RuntimeActivityRow, state: &InlineState) -> bool {
    if state.debug {
        return !activity_row_is_shell_output_or_completion(row);
    }
    should_render_activity_row(row, state.approval_mode)
}

fn activity_row_is_shell_output_or_completion(row: &RuntimeActivityRow) -> bool {
    match row.kind {
        ActivityKind::ToolOutput => row.detail.contains("provider_native_shell_command: "),
        ActivityKind::Tool => {
            activity_row_is_shell_tool(row)
                && matches!(row.status.as_str(), "success" | "completed")
        }
        _ => false,
    }
}

fn activity_row_is_shell_tool(row: &RuntimeActivityRow) -> bool {
    row.detail
        .lines()
        .find_map(|line| line.strip_prefix("tool_name: "))
        .is_some_and(is_shell_tool_name)
        || row.detail.contains("provider_native_shell_command: ")
        || row.detail.contains("provider_tool_class: shell")
        || row.subject == "Bash"
}

fn activity_row_is_question_tool(row: &RuntimeActivityRow) -> bool {
    row.detail
        .lines()
        .find_map(|line| line.strip_prefix("tool_name: "))
        .is_some_and(|name| {
            matches!(
                name,
                "ask_user_question" | "AskUserQuestion" | "ask_user" | "AskUser"
            )
        })
}

fn activity_row_is_control_permission(row: &RuntimeActivityRow) -> bool {
    row.detail.contains("evidence: ProviderToolRequest")
}

fn activity_detail_for_render(row: &RuntimeActivityRow, debug: bool) -> String {
    if debug {
        return row.detail.clone();
    }
    row.detail
        .lines()
        .filter(|line| !line.starts_with("debug_output_ref: "))
        .collect::<Vec<_>>()
        .join("\n")
}

impl ActivityKind {
    fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::ToolOutput => "output",
            Self::Tool => "tool",
            Self::ShellHandoff => "shell",
        }
    }
}
