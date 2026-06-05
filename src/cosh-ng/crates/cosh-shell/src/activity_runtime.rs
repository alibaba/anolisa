use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use cosh_shell::{
    agent_render::{ActivityDetailsPanelModel, ActivityPanelModel, ActivityRowModel},
    AgentEvent, GovernedEvent, RatatuiInlineRenderer,
};

use super::*;

#[derive(Debug, Clone)]
pub(super) struct RuntimeActivityRow {
    pub(super) id: String,
    pub(super) run_id: String,
    pub(super) kind: ActivityKind,
    pub(super) status: String,
    pub(super) subject: String,
    pub(super) summary: String,
    pub(super) detail: String,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ActivityKind {
    Skill,
    ToolOutput,
    Tool,
}

pub(super) fn record_activity_rows(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
) -> Vec<String> {
    let mut ids = Vec::new();
    for event in governed_events {
        let row = match &event.event {
            AgentEvent::SkillLoadStarted {
                run_id,
                skill,
                reason,
            } => Some(RuntimeActivityRow {
                id: next_activity_id(state, "skill"),
                run_id: run_id.clone(),
                kind: ActivityKind::Skill,
                status: "loading".to_string(),
                subject: skill.clone(),
                summary: format!("{skill} loading"),
                detail: format!("skill: {skill}\nstatus: loading\nreason: {reason}"),
            }),
            AgentEvent::SkillLoadCompleted {
                run_id,
                skill,
                summary,
            } => Some(RuntimeActivityRow {
                id: next_activity_id(state, "skill"),
                run_id: run_id.clone(),
                kind: ActivityKind::Skill,
                status: "loaded".to_string(),
                subject: skill.clone(),
                summary: format!("{skill} loaded"),
                detail: format!("skill: {skill}\nstatus: loaded\nsummary: {summary}"),
            }),
            AgentEvent::SkillLoadFailed {
                run_id,
                skill,
                error,
            } => Some(RuntimeActivityRow {
                id: next_activity_id(state, "skill"),
                run_id: run_id.clone(),
                kind: ActivityKind::Skill,
                status: "failed".to_string(),
                subject: skill.clone(),
                summary: format!("{skill} failed"),
                detail: format!("skill: {skill}\nstatus: failed\nerror: {error}"),
            }),
            AgentEvent::ToolOutputDelta {
                run_id,
                tool_id,
                stream,
                text,
            } => Some(tool_output_row(state, run_id, tool_id, stream, text)),
            AgentEvent::ToolCompleted {
                run_id,
                tool_id,
                status,
            } => Some(RuntimeActivityRow {
                id: next_activity_id(state, "tool"),
                run_id: run_id.clone(),
                kind: ActivityKind::Tool,
                status: status.clone(),
                subject: tool_id.clone(),
                summary: status.clone(),
                detail: format!("tool: {tool_id}\nstatus: {status}"),
            }),
            _ => None,
        };
        if let Some(row) = row {
            let id = row.id.clone();
            state.activity_rows.push(row);
            ids.push(id);
        }
    }
    ids
}

fn tool_output_row(
    state: &mut InlineState,
    run_id: &str,
    tool_id: &str,
    stream: &str,
    text: &str,
) -> RuntimeActivityRow {
    let id = next_activity_id(state, "out");
    let output_ref = state
        .activity_output_dir
        .as_deref()
        .and_then(|dir| write_tool_output_ref(dir, &id, text).ok())
        .map(|path| path.display().to_string());
    RuntimeActivityRow {
        id: id.clone(),
        run_id: run_id.to_string(),
        kind: ActivityKind::ToolOutput,
        status: "captured".to_string(),
        subject: tool_id.to_string(),
        summary: format!("{stream} captured; /details {id}"),
        detail: tool_output_detail(
            tool_id,
            stream,
            text.lines().count(),
            output_ref.as_deref(),
            text,
        ),
    }
}

pub(super) fn write_tool_output_ref(dir: &Path, id: &str, text: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{id}.txt"));
    fs::write(&path, text)?;
    Ok(path)
}

fn tool_output_detail(
    tool_id: &str,
    stream: &str,
    lines: usize,
    output_ref: Option<&str>,
    text: &str,
) -> String {
    let mut detail = format!("tool: {tool_id}\nstream: {stream}\nlines: {lines}");
    if let Some(output_ref) = output_ref {
        detail.push_str(&format!("\nref: {output_ref}"));
    }
    detail.push('\n');
    detail.push_str(text);
    detail
}

pub(super) fn next_activity_id(state: &InlineState, prefix: &str) -> String {
    let prefix_with_dash = format!("{prefix}-");
    let next = state
        .activity_rows
        .iter()
        .filter(|row| row.id.starts_with(&prefix_with_dash))
        .count()
        + 1;
    format!("{prefix}-{next}")
}

pub(super) fn render_activity_rows<W: Write>(
    state: &InlineState,
    activity_ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    let rows = activity_ids
        .iter()
        .filter_map(|activity_id| {
            state
                .activity_rows
                .iter()
                .find(|row| row.id == *activity_id)
        })
        .filter(|row| row.status != "loading")
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
        .write_activity_panel(output, ActivityPanelModel { rows })?;
    Ok(())
}

pub(super) fn render_activity_details<W: Write>(
    row: &RuntimeActivityRow,
    output: &mut W,
) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal().write_activity_details_panel(
        output,
        ActivityDetailsPanelModel {
            id: &row.id,
            run_id: &row.run_id,
            kind: row.kind.label(),
            status: &row.status,
            subject: &row.subject,
            summary: &row.summary,
            detail: &row.detail,
        },
    )?;
    Ok(())
}

pub(super) fn render_activity_details_by_id<W: Write>(
    state: &InlineState,
    id: &str,
    output: &mut W,
) -> Option<std::io::Result<()>> {
    state
        .activity_rows
        .iter()
        .find(|row| row.id == id)
        .map(|row| render_activity_details(row, output))
}

impl ActivityKind {
    fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::ToolOutput => "output",
            Self::Tool => "tool",
        }
    }
}
