use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cosh_shell::exit_classify::classify_shell_handoff_command_outcome;
use cosh_shell::tools::display::display_for_tool;
use cosh_shell::tools::is_shell_tool_name;
use cosh_shell::{
    agent_render::{
        ActivityDetailsPanelModel, ActivityPanelModel, ActivityRowModel, RatatuiInlineRenderer,
    },
    types::{AgentEvent, GovernedEvent},
    MessageId,
};

use crate::runtime::evidence_delivery::record_shell_handoff_completion;
use crate::runtime::state::PendingInteractiveShellHandoff;

use crate::runtime::prelude::*;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeActivityRow {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) kind: ActivityKind,
    pub(crate) status: String,
    pub(crate) subject: String,
    pub(crate) summary: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ActivityKind {
    Skill,
    ToolOutput,
    Tool,
    ShellHandoff,
}

#[cfg(test)]
fn record_activity_rows(state: &mut InlineState, governed_events: &[GovernedEvent]) -> Vec<String> {
    record_activity_rows_with_policy(state, governed_events, ActivityRecordPolicy::default())
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ActivityRecordPolicy {
    pub(crate) suppress_provider_native_shell: bool,
}

pub(crate) fn record_activity_rows_with_policy(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    policy: ActivityRecordPolicy,
) -> Vec<String> {
    let mut ids = Vec::new();
    let permission_tool_use_ids = governed_events
        .iter()
        .filter_map(|event| match &event.event {
            AgentEvent::ToolPermissionRequest { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
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
                summary: activity_summary_message(
                    state,
                    MessageId::ActivitySkillLoadingSummary,
                    &[("skill", skill)],
                ),
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
                summary: activity_summary_message(
                    state,
                    MessageId::ActivitySkillLoadedSummary,
                    &[("skill", skill)],
                ),
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
                summary: activity_summary_message(
                    state,
                    MessageId::ActivitySkillFailedSummary,
                    &[("skill", skill)],
                ),
                detail: format!("skill: {skill}\nstatus: failed\nerror: {error}"),
            }),
            AgentEvent::ToolCall {
                run_id,
                tool_id,
                name,
                input,
            } => {
                let covered_by_control_permission = tool_id
                    .as_deref()
                    .is_some_and(|tool_id| permission_tool_use_ids.contains(tool_id));
                if is_shell_tool_name(name) {
                    if let Some(tool_id) = tool_id.as_deref() {
                        state
                            .control
                            .record_provider_shell_command_from_tool_call(run_id, tool_id, input);
                    } else {
                        state
                            .control
                            .record_pending_provider_shell_command(run_id, input);
                    }
                    if covered_by_control_permission {
                        continue;
                    }
                    let provider_shell_command =
                        provider_shell_command_for_tool_call(state, tool_id.as_deref(), input);
                    if policy.suppress_provider_native_shell {
                        if provider_shell_transcript_seen(state, tool_id.as_deref()) {
                            continue;
                        }
                        if provider_shell_command.as_deref().is_some_and(|command| {
                            state
                                .control
                                .provider_foreground_shell_command_seen(command)
                        }) {
                            if let Some(tool_id) = tool_id.as_deref() {
                                state.control.mark_provider_shell_transcript_seen(tool_id);
                            }
                            continue;
                        }
                        Some(provider_native_shell_auto_approved_row(
                            state,
                            run_id,
                            tool_id.as_deref(),
                            name,
                            input,
                            None,
                        ))
                    } else {
                        Some(provider_tool_call_row(
                            state,
                            run_id,
                            tool_id.as_deref(),
                            name,
                            input,
                        ))
                    }
                } else {
                    if covered_by_control_permission {
                        continue;
                    }
                    Some(provider_tool_call_row(
                        state,
                        run_id,
                        tool_id.as_deref(),
                        name,
                        input,
                    ))
                }
            }
            AgentEvent::ToolOutputDelta {
                run_id,
                tool_id,
                stream,
                text,
            } => {
                state
                    .control
                    .record_provider_tool_output_delta(run_id, tool_id, stream, text);
                if state
                    .control
                    .provider_tool_is_control_permission_shell(tool_id)
                    || (state.control.provider_tool_is_shell(tool_id)
                        && state.control.provider_shell_transcript_seen(tool_id))
                {
                    continue;
                } else {
                    Some(tool_output_row(state, run_id, tool_id, stream, text))
                }
            }
            AgentEvent::ToolPermissionRequest {
                run_id,
                request_id,
                tool_name,
                tool_input,
                tool_use_id,
            } => {
                state.control.record_provider_tool_command_from_input(
                    run_id,
                    tool_use_id,
                    tool_input,
                );
                if is_shell_tool_name(tool_name) {
                    state
                        .control
                        .mark_provider_control_permission_shell_tool(tool_use_id);
                }
                Some(provider_tool_request_row(
                    state,
                    run_id,
                    request_id,
                    tool_name,
                    tool_input,
                    tool_use_id,
                ))
            }
            AgentEvent::ToolCompleted {
                run_id,
                tool_id,
                status,
            } => {
                if state
                    .control
                    .provider_tool_is_control_permission_shell(tool_id)
                    || (state.control.provider_tool_is_shell(tool_id)
                        && state.control.provider_shell_transcript_seen(tool_id))
                {
                    continue;
                } else {
                    Some(tool_completed_row(state, run_id, tool_id, status))
                }
            }
            _ => None,
        };
        if let Some(row) = row {
            let id = row.id.clone();
            state.activity.rows.push(row);
            ids.push(id);
        }
    }
    ids
}

fn provider_shell_transcript_seen(state: &InlineState, tool_id: Option<&str>) -> bool {
    tool_id.is_some_and(|tool_id| state.control.provider_shell_transcript_seen(tool_id))
}

fn provider_shell_command_for_tool_call(
    state: &InlineState,
    tool_id: Option<&str>,
    input: &str,
) -> Option<String> {
    tool_id
        .and_then(|tool_id| state.control.provider_tool().command(tool_id))
        .map(|command| command.command.clone())
        .or_else(|| shell_command_from_tool_call_input(input))
}

fn shell_command_from_tool_call_input(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() || input.contains('\0') {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|value| {
            value
                .get("command")
                .and_then(|command| command.as_str())
                .filter(|command| !command.is_empty() && !command.contains('\0'))
                .map(ToString::to_string)
        })
        .or_else(|| Some(input.to_string()))
}

fn provider_native_shell_auto_approved_row(
    state: &mut InlineState,
    run_id: &str,
    tool_id: Option<&str>,
    tool_name: &str,
    input: &str,
    artifact: Option<(&str, &str)>,
) -> RuntimeActivityRow {
    let id = next_activity_id(state, "tool");
    let subject = tool_id.unwrap_or(tool_name).to_string();
    let command = tool_id
        .and_then(|tool_id| state.control.provider_tool().command(tool_id))
        .map(|command| command.command.as_str())
        .unwrap_or(input);
    let mut detail = format!(
        "evidence: ProviderNativeShellBypass\nprovider: provider_native_stream\nexecution_path: provider_native_shell_bypassed_control_protocol\ntool_id: {}\ntool_name: {tool_name}\nprovider_native_shell_command: {}\ninput_preview: {}\nprovider_auto_approval_status: auto_approved_by_provider\nreason: control_protocol_provider_emitted_shell_tool_without_foreground_handoff",
        tool_id.unwrap_or("<none>"),
        truncate_activity_preview(command, 4_000),
        truncate_activity_preview(input, 4_000)
    );
    if let Some((kind, text)) = artifact {
        detail.push_str(&format!(
            "\nartifact_kind: {kind}\nartifact_preview:\n{}",
            truncate_activity_preview(text, 4_000)
        ));
    }
    let preview = activity_summary_preview(&format!("$ {command}"), 120);
    RuntimeActivityRow {
        id: id.clone(),
        run_id: run_id.to_string(),
        kind: ActivityKind::Tool,
        status: "auto-approved".to_string(),
        subject,
        summary: activity_summary_message(
            state,
            MessageId::ActivityProviderNativeShellBypassSummary,
            &[("tool", tool_name), ("preview", &preview), ("id", &id)],
        ),
        detail,
    }
}

fn provider_tool_call_row(
    state: &mut InlineState,
    run_id: &str,
    tool_id: Option<&str>,
    tool_name: &str,
    input: &str,
) -> RuntimeActivityRow {
    let id = next_activity_id(state, "tool");
    let info = display_for_tool(tool_name, input);
    RuntimeActivityRow {
        id: id.clone(),
        run_id: run_id.to_string(),
        kind: ActivityKind::Tool,
        status: "called".to_string(),
        subject: tool_id.unwrap_or(&info.label).to_string(),
        summary: activity_summary_message(
            state,
            MessageId::ActivityToolCalledSummary,
            &[
                ("tool", tool_name),
                ("preview", &activity_summary_preview(&info.preview, 120)),
                ("id", &id),
            ],
        ),
        detail: format!(
            "evidence: ProviderToolCall\nprovider: provider_native_stream\nexecution_path: provider_native_stream\ntool_id: {}\ntool_name: {tool_name}\ninput_preview: {}\nagent_result_visibility: provider_native_result",
            tool_id.unwrap_or("<none>"),
            info.preview
        ),
    }
}

fn provider_tool_request_row(
    state: &mut InlineState,
    run_id: &str,
    request_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    tool_use_id: &str,
) -> RuntimeActivityRow {
    let id = next_activity_id(state, "tool");
    let input_str = serde_json::to_string(tool_input).unwrap_or_default();
    let info = display_for_tool(tool_name, &input_str);
    let preview = provider_tool_input_preview(tool_name, tool_input, &info.preview);
    RuntimeActivityRow {
        id: id.clone(),
        run_id: run_id.to_string(),
        kind: ActivityKind::Tool,
        status: "requested".to_string(),
        subject: tool_use_id.to_string(),
        summary: activity_summary_message(
            state,
            MessageId::ActivityToolRequestedSummary,
            &[
                ("tool", &info.label),
                ("preview", &activity_summary_preview(&preview, 120)),
                ("id", &id),
            ],
        ),
        detail: format!(
            "evidence: ProviderToolRequest\nprovider: provider_control_protocol\nexecution_path: provider_control_protocol\nrequest_id: {request_id}\ntool_use_id: {tool_use_id}\ntool_name: {tool_name}\ninput_preview: {preview}\nagent_result_visibility: provider_native_result"
        ),
    }
}

fn provider_tool_input_preview(
    tool_name: &str,
    tool_input: &serde_json::Value,
    display_preview: &str,
) -> String {
    let preview = if is_shell_tool_name(tool_name) {
        tool_input
            .get("command")
            .and_then(|value| value.as_str())
            .map(|command| format!("$ {command}"))
            .unwrap_or_else(|| display_preview.to_string())
    } else {
        display_preview.to_string()
    };
    truncate_activity_preview(&preview, 4_000)
}

fn tool_completed_row(
    state: &mut InlineState,
    run_id: &str,
    tool_id: &str,
    status: &str,
) -> RuntimeActivityRow {
    let id = next_activity_id(state, "tool");
    let interactive_handoff = maybe_queue_interactive_shell_handoff(state, tool_id, status);
    let stderr = state.control.provider_tool().stderr(tool_id);
    let stderr_summary = stderr.and_then(first_error_line);
    let mut summary = if matches!(status, "error" | "failed" | "interrupted") {
        match stderr_summary.as_deref() {
            Some(line) => format!("{line}; [Details] {id}"),
            None => format!("[Details] {id}"),
        }
    } else {
        status.to_string()
    };
    let mut detail = format!("tool: {tool_id}\nstatus: {status}");
    if let Some(command) = state.control.provider_tool().command(tool_id) {
        detail.push_str(&format!(
            "\nprovider_native_shell_command: {}",
            command.command
        ));
    }
    if let Some(stderr) = stderr {
        detail.push_str("\nstderr:\n");
        detail.push_str(stderr);
    }
    if let Some(handoff) = interactive_handoff {
        let handoff_summary = activity_summary_message(
            state,
            MessageId::ActivityToolNeedsForegroundShellSummary,
            &[("handoff", &handoff.id), ("id", &id)],
        );
        summary = match stderr_summary.as_deref() {
            Some(line) => format!("{line}; {handoff_summary}"),
            None => handoff_summary,
        };
        detail.push_str(&format!(
            "\ninteractive_hint: may_require_foreground_shell\nsend_to_shell_action: {}\nexact_command: {}\nprovider_tool_id: {}\nfollow_up: start a new Agent turn after the shell command completes if analysis is needed",
            handoff.id, handoff.command, handoff.tool_id
        ));
    }
    RuntimeActivityRow {
        id,
        run_id: run_id.to_string(),
        kind: ActivityKind::Tool,
        status: status.to_string(),
        subject: tool_id.to_string(),
        summary,
        detail,
    }
}

fn first_error_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| truncate_activity_preview(line, 160))
}

fn maybe_queue_interactive_shell_handoff(
    state: &mut InlineState,
    tool_id: &str,
    status: &str,
) -> Option<PendingInteractiveShellHandoff> {
    state
        .control
        .queue_interactive_shell_handoff_for_tool_failure(tool_id, status)
}

fn truncate_activity_preview(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}... <truncated>")
    } else {
        truncated
    }
}

fn activity_summary_preview(value: &str, max_chars: usize) -> String {
    truncate_activity_preview(&value.replace('\n', "\\n"), max_chars)
}

pub(crate) fn record_approved_shell_handoff_blocks(
    state: &mut InlineState,
    blocks: &[CommandBlock],
) -> Vec<String> {
    let mut ids = Vec::new();
    while let Some(handoff) = state.control.shell_handoff().pending_front() {
        let request = handoff.request();
        let Some(block) = blocks.iter().find(|block| {
            block.command == request.command
                && (block.ended_at_ms >= request.created_at_ms
                    || request.source == "approved_provider_shell_tool"
                    || request.approval_id.starts_with("handoff-"))
        }) else {
            break;
        };

        let handoff = state
            .control
            .shell_handoff_mut()
            .pop_pending()
            .expect("front handoff exists");
        let handoff_request = handoff.request();
        let id = next_shell_handoff_activity_id(state, &handoff_request.approval_id);
        let status = classify_shell_handoff_command_outcome(
            block.exit_code,
            &block.command,
            handoff.timeout_interrupt_sent(),
        )
        .status();
        state
            .approvals
            .mark_foreground_shell_execution(&handoff_request.approval_id, &block.id);
        state
            .control
            .mark_provider_foreground_shell_command(&block.command);
        let evidence = record_shell_handoff_completion(state, handoff_request, block, status);
        if let Some(tool_use_id) = handoff_request.tool_use_id.as_deref() {
            state
                .control
                .mark_provider_shell_transcript_seen(tool_use_id);
        }
        state
            .analyzed_blocks
            .insert(evidence.command_block_id.clone());
        state.activity.rows.push(RuntimeActivityRow {
            id: id.clone(),
            run_id: handoff_request.run_id.clone(),
            kind: ActivityKind::ShellHandoff,
            status: evidence.status.to_string(),
            subject: evidence.approval_id.clone().unwrap_or_default(),
            summary: activity_summary_message(
                state,
                MessageId::ActivityShellHandoffSentSummary,
                &[("approval", &handoff_request.approval_id)],
            ),
            detail: format!(
                "evidence: ShellCommandCompleted\napproval: {}\nexecution_path: foreground_shell_pty\nselected_shell_execution_path: {}\npath_selection_reason: {}\nprovider_result_delivery_status: {}\nrecovery_reason: {}\ncommand_block: {}\ncommand: {}\ncwd: {}\nend_cwd: {}\npreview: {}\npreview_hash: {}\nactor: {}\nsource: {}\nrequest_id: {}\ntool_use_id: {}\nstatus: {}\nexit_code: {}\nduration_ms: {}\nredaction_status: {}\noutput_id: {}",
                evidence.approval_id.as_deref().unwrap_or("<none>"),
                evidence.selected_execution_path(),
                evidence.path_selection_reason(),
                evidence.provider_result_delivery_status,
                evidence.recovery_reason.unwrap_or("<none>"),
                evidence.command_block_id,
                evidence.command,
                evidence.cwd,
                evidence.end_cwd,
                handoff_request.exact_preview,
                handoff_request.preview_hash,
                handoff_request.actor,
                handoff_request.source,
                handoff_request.request_id.as_deref().unwrap_or("<none>"),
                handoff_request.tool_use_id.as_deref().unwrap_or("<none>"),
                evidence.status,
                evidence.exit_code,
                evidence.duration_ms,
                evidence.redaction_status,
                evidence.terminal_output_ref.as_ref().map_or_else(
                    || "<none>".to_string(),
                    |_| crate::evidence::output_policy::terminal_output_id(
                        &evidence.shell_session_id,
                        &evidence.command_block_id
                    )
                )
            ),
        });
        ids.push(id);
    }
    ids
}

fn next_shell_handoff_activity_id(state: &InlineState, approval_id: &str) -> String {
    if approval_id.starts_with("handoff-")
        && !state.activity.rows.iter().any(|row| row.id == approval_id)
    {
        return approval_id.to_string();
    }

    let reserved_handoff_ids = state.control.interactive_shell_handoff_ids();
    next_activity_id_excluding(state, "handoff", reserved_handoff_ids)
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
        .activity
        .output_dir
        .as_deref()
        .and_then(|dir| write_tool_output_ref(dir, &id, text).ok())
        .map(|path| path.display().to_string());
    let provider_native_shell_command = state
        .control
        .provider_tool()
        .command(tool_id)
        .map(|command| command.command.as_str());
    let provider_shell_tool = state.control.provider_tool_is_shell(tool_id);
    RuntimeActivityRow {
        id: id.clone(),
        run_id: run_id.to_string(),
        kind: ActivityKind::ToolOutput,
        status: "captured".to_string(),
        subject: tool_id.to_string(),
        summary: tool_output_summary(state, stream, &id),
        detail: tool_output_detail(
            tool_id,
            stream,
            text.lines().count(),
            output_ref.as_deref(),
            text,
            provider_native_shell_command,
            provider_shell_tool,
        ),
    }
}

pub(crate) fn write_tool_output_ref(dir: &Path, id: &str, text: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{id}.txt"));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(path)
}

fn tool_output_detail(
    tool_id: &str,
    stream: &str,
    lines: usize,
    output_ref: Option<&str>,
    text: &str,
    provider_native_shell_command: Option<&str>,
    provider_shell_tool: bool,
) -> String {
    let mut detail = format!("tool: {tool_id}\nstream: {stream}\nlines: {lines}");
    if provider_shell_tool {
        detail.push_str("\nprovider_tool_class: shell");
    }
    if let Some(command) = provider_native_shell_command {
        detail.push_str(&format!("\nprovider_native_shell_command: {command}"));
    }
    let capture_status = if output_ref.is_some() {
        "captured"
    } else {
        "unavailable"
    };
    if let Some(output_ref) = output_ref {
        detail.push_str(&format!("\ndebug_output_ref: {output_ref}"));
    }
    detail.push_str(&format!(
        "\ncapture_status: {capture_status}\noutput_ref: <hidden>"
    ));
    detail.push('\n');
    detail.push_str(text);
    detail
}

pub(crate) fn next_activity_id(state: &InlineState, prefix: &str) -> String {
    next_activity_id_excluding(state, prefix, std::iter::empty())
}

fn next_activity_id_excluding<'a>(
    state: &'a InlineState,
    prefix: &str,
    excluded_ids: impl IntoIterator<Item = &'a str>,
) -> String {
    let prefix_with_dash = format!("{prefix}-");
    let mut used_ids = state
        .activity
        .rows
        .iter()
        .filter(|row| row.id.starts_with(&prefix_with_dash))
        .map(|row| row.id.clone())
        .collect::<HashSet<_>>();
    used_ids.extend(excluded_ids.into_iter().map(str::to_string));

    let mut next = 1;
    loop {
        let id = format!("{prefix}-{next}");
        if !used_ids.contains(&id) {
            return id;
        }
        next += 1;
    }
}

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

pub(crate) fn render_activity_details<W: Write>(
    language: cosh_shell::Language,
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

fn activity_summary_message(state: &InlineState, id: MessageId, args: &[(&str, &str)]) -> String {
    state.i18n().format(id, args)
}

fn tool_output_summary(state: &InlineState, stream: &str, id: &str) -> String {
    let message_id = match stream {
        "stdout" => MessageId::ToolOutputStdoutCapturedSummary,
        "stderr" => MessageId::ToolOutputStderrCapturedSummary,
        _ => MessageId::ActivityToolOutputCapturedSummary,
    };
    activity_summary_message(state, message_id, &[("stream", stream), ("id", id)])
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn governed(event: AgentEvent) -> GovernedEvent {
        GovernedEvent {
            decision: cosh_shell::types::GovernanceDecision::Display,
            policy_decision: cosh_shell::types::GovernancePolicyDecision::DisplayOnly,
            event,
            reason: "test".to_string(),
            display_text: "test".to_string(),
            auto_execute: false,
        }
    }

    #[test]
    fn activity_tool_output_summary_uses_state_language() {
        let mut state = InlineState {
            language: cosh_shell::Language::ZhCn,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "tool-1".to_string(),
                stream: "stdout".to_string(),
                text: "line 1\nline 2".to_string(),
            })],
        );

        assert_eq!(ids, vec!["out-1"]);
        let row = state
            .activity
            .rows
            .iter()
            .find(|row| row.id == "out-1")
            .expect("activity row");
        assert_eq!(row.summary, "stdout 已捕获；[Details] out-1");
        assert!(row.detail.contains("stream: stdout"));

        let mut output = Vec::new();
        render_activity_details_by_id(&state, "out-1", &mut output)
            .expect("details result")
            .expect("render details");
        let output = String::from_utf8(output).expect("utf8 output");
        assert!(output.contains("活动详情 out-1"), "{output}");
        assert!(output.contains("运行: run-1"), "{output}");
        assert!(output.contains("详情:"), "{output}");
        assert!(output.contains("stream: stdout"), "{output}");
    }

    #[test]
    fn activity_tool_output_details_hide_internal_output_ref_path() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-activity-details-hide-ref-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = InlineState::with_raw_session_dir(&dir);
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "tool-1".to_string(),
                stream: "stdout".to_string(),
                text: "secret-ish\n".to_string(),
            })],
        );

        assert_eq!(ids, vec!["out-1"]);
        let output_ref = dir.join("agent-output-refs/out-1.txt");
        assert!(output_ref.exists(), "output ref should still be captured");

        let mut output = Vec::new();
        render_activity_details_by_id(&state, "out-1", &mut output)
            .expect("details result")
            .expect("render details");
        let output = String::from_utf8(output).expect("utf8 output");
        assert!(output.contains("capture_status: captured"), "{output}");
        assert!(output.contains("output_ref: <hidden>"), "{output}");
        assert!(!output.contains(output_ref.to_str().unwrap()), "{output}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn activity_tool_output_details_show_internal_output_ref_in_debug() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-activity-details-debug-ref-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = InlineState {
            debug: true,
            ..InlineState::with_raw_session_dir(&dir)
        };
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "tool-1".to_string(),
                stream: "stdout".to_string(),
                text: "debug-visible\n".to_string(),
            })],
        );

        assert_eq!(ids, vec!["out-1"]);
        let output_ref = dir.join("agent-output-refs/out-1.txt");
        assert!(output_ref.exists(), "output ref should be captured");

        let mut output = Vec::new();
        render_activity_details_by_id(&state, "out-1", &mut output)
            .expect("details result")
            .expect("render details");
        let output = String::from_utf8(output).expect("utf8 output");
        assert!(output.contains("debug_output_ref:"), "{output}");
        assert!(output.contains("out-1.txt"), "{output}");
        assert!(output.contains("output_ref: <hidden>"), "{output}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_shell_provider_tool_call_renders_activity_card() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: None,
                name: "Read".to_string(),
                input: r#"{"file_path":"Cargo.toml"}"#.to_string(),
            })],
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("Activity"), "{output}");
        assert!(
            output.contains("Read called: Cargo.toml; [Details] tool-1"),
            "{output}"
        );
    }

    #[test]
    fn shell_provider_tool_call_still_uses_shell_visibility_path() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: None,
                name: "run_shell_command".to_string(),
                input: "df -h".to_string(),
            })],
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(!output.contains("Activity"), "{output}");
        assert!(!output.contains("run_shell_command called"), "{output}");
    }

    #[test]
    fn provider_native_shell_output_renders_transcript_without_activity_card() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-1".to_string()),
                    name: "run_shell_command".to_string(),
                    input: serde_json::json!({ "command": "df -h" }).to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-1".to_string(),
                    stream: "stdout".to_string(),
                    text: "Filesystem\n/dev/disk1\n".to_string(),
                }),
                governed(AgentEvent::ToolCompleted {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-1".to_string(),
                    status: "completed".to_string(),
                }),
            ],
        );

        let mut output = Vec::new();
        render_provider_native_shell_transcript(&mut state, &ids, &mut output)
            .expect("render shell transcript");
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("$ df -h"), "{output}");
        assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
        assert!(!output.contains("Activity"), "{output}");
        assert!(
            !output.contains("stdout captured; [Details] out-1"),
            "{output}"
        );
        assert!(!output.contains("Tool completed"), "{output}");
        let detail = &state
            .activity
            .rows
            .iter()
            .find(|row| row.id == "out-1")
            .expect("output row")
            .detail;
        assert!(
            detail.contains("provider_native_shell_command: df -h"),
            "{detail}"
        );
    }

    #[test]
    fn provider_native_shell_transcript_uses_structured_tool_state() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-1".to_string()),
                    name: "run_shell_command".to_string(),
                    input: serde_json::json!({ "command": "df -h" }).to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-1".to_string(),
                    stream: "stdout".to_string(),
                    text: "Filesystem\n/dev/disk1\n".to_string(),
                }),
            ],
        );
        let row = state
            .activity
            .rows
            .iter_mut()
            .find(|row| row.id == "out-1")
            .expect("output row");
        row.detail =
            "tool: toolu-1\nstream: stdout\noutput_ref: <hidden>\nDETAIL_ONLY_SHOULD_NOT_RENDER\n"
                .to_string();

        let mut output = Vec::new();
        render_provider_native_shell_transcript(&mut state, &ids, &mut output)
            .expect("render shell transcript");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("$ df -h"), "{output}");
        assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
        assert!(
            !output.contains("DETAIL_ONLY_SHOULD_NOT_RENDER"),
            "{output}"
        );
    }

    #[test]
    fn provider_native_streamed_shell_output_renders_transcript_without_control_permission() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-shell".to_string()),
                    name: "run_shell_command".to_string(),
                    input: "df -h".to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-shell".to_string(),
                    stream: "stdout".to_string(),
                    text: "Filesystem\n/dev/disk1\n".to_string(),
                }),
                governed(AgentEvent::ToolCompleted {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-shell".to_string(),
                    status: "success".to_string(),
                }),
            ],
        );

        let mut output = Vec::new();
        render_provider_native_shell_transcript(&mut state, &ids, &mut output)
            .expect("render shell transcript");
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("$ df -h"), "{output}");
        assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
        assert!(!output.contains("Activity"), "{output}");
        assert!(
            !output.contains("stdout captured; [Details] out-1"),
            "{output}"
        );
        let detail = &state
            .activity
            .rows
            .iter()
            .find(|row| row.id == "out-1")
            .expect("output row")
            .detail;
        assert!(
            detail.contains("provider_native_shell_command: df -h"),
            "{detail}"
        );
    }

    #[test]
    fn control_protocol_policy_suppresses_provider_auto_approved_shell_activity() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows_with_policy(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-shell".to_string()),
                    name: "run_shell_command".to_string(),
                    input: "df -h".to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-shell".to_string(),
                    stream: "stdout".to_string(),
                    text: "Filesystem\n/dev/disk1\n".to_string(),
                }),
                governed(AgentEvent::ToolCompleted {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-shell".to_string(),
                    status: "success".to_string(),
                }),
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-read".to_string()),
                    name: "Read".to_string(),
                    input: r#"{"file_path":"Cargo.toml"}"#.to_string(),
                }),
            ],
            ActivityRecordPolicy {
                suppress_provider_native_shell: true,
            },
        );

        let mut output = Vec::new();
        render_provider_native_shell_transcript(&mut state, &ids, &mut output)
            .expect("render shell transcript");
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("$ df -h"), "{output}");
        assert!(output.contains("Filesystem\n/dev/disk1\n"), "{output}");
        assert!(
            !output.contains("run_shell_command auto-approved by provider"),
            "{output}"
        );
        assert!(
            output.contains("Read called: Cargo.toml; [Details]"),
            "{output}"
        );
        assert!(state.activity.rows.iter().any(|row| {
            row.detail.contains("evidence: ProviderNativeShellBypass")
                && row
                    .detail
                    .contains("provider_native_shell_bypassed_control_protocol")
                && row
                    .detail
                    .contains("provider_auto_approval_status: auto_approved_by_provider")
                && row.detail.contains("provider_native_shell_command: df -h")
        }));
    }

    #[test]
    fn debug_mode_keeps_provider_auto_approved_shell_activity() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            debug: true,
            ..InlineState::default()
        };
        let ids = record_activity_rows_with_policy(
            &mut state,
            &[governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-shell".to_string()),
                name: "run_shell_command".to_string(),
                input: "df -h".to_string(),
            })],
            ActivityRecordPolicy {
                suppress_provider_native_shell: true,
            },
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(
            output.contains("run_shell_command auto-approved by provider: $ df -h; [Details]"),
            "{output}"
        );
    }

    #[test]
    fn question_tool_call_is_hidden_when_question_card_handles_it() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("toolu-question".to_string()),
                name: "ask_user_question".to_string(),
                input: serde_json::json!({
                    "question": "Pick one",
                    "options": [{"label": "A"}, {"label": "B"}]
                })
                .to_string(),
            })],
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(!output.contains("Activity"), "{output}");
        assert!(!output.contains("ask_user_question called"), "{output}");
    }

    #[test]
    fn control_permission_tool_request_is_hidden_when_approval_card_handles_it() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            approval_mode: CoshApprovalMode::Auto,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolPermissionRequest {
                run_id: "run-1".to_string(),
                request_id: "ctrl-write".to_string(),
                tool_name: "Write".to_string(),
                tool_input: serde_json::json!({
                    "file_path": "/tmp/cosh-write.txt",
                    "content": "ok"
                }),
                tool_use_id: "toolu-write".to_string(),
            })],
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(!output.contains("Activity"), "{output}");
        assert!(!output.contains("Write requested"), "{output}");
    }

    #[test]
    fn matching_tool_call_is_hidden_when_control_permission_card_handles_it() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            approval_mode: CoshApprovalMode::Auto,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-write".to_string()),
                    name: "Write".to_string(),
                    input: serde_json::json!({
                        "file_path": "/tmp/cosh-write.txt",
                        "content": "ok"
                    })
                    .to_string(),
                }),
                governed(AgentEvent::ToolPermissionRequest {
                    run_id: "run-1".to_string(),
                    request_id: "ctrl-write".to_string(),
                    tool_name: "Write".to_string(),
                    tool_input: serde_json::json!({
                        "file_path": "/tmp/cosh-write.txt",
                        "content": "ok"
                    }),
                    tool_use_id: "toolu-write".to_string(),
                }),
            ],
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(!output.contains("Activity"), "{output}");
        assert!(!output.contains("Write called"), "{output}");
        assert!(!output.contains("Write requested"), "{output}");
    }

    #[test]
    fn recommend_mode_keeps_only_control_permission_row_for_matching_tool_call() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            approval_mode: CoshApprovalMode::Recommend,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-read".to_string()),
                    name: "Read".to_string(),
                    input: serde_json::json!({ "file_path": "Cargo.toml" }).to_string(),
                }),
                governed(AgentEvent::ToolPermissionRequest {
                    run_id: "run-1".to_string(),
                    request_id: "ctrl-read".to_string(),
                    tool_name: "Read".to_string(),
                    tool_input: serde_json::json!({ "file_path": "Cargo.toml" }),
                    tool_use_id: "toolu-read".to_string(),
                }),
            ],
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(!output.contains("Read called"), "{output}");
        assert!(
            output.contains("Read requested: Cargo.toml; [Details]"),
            "{output}"
        );
    }

    #[test]
    fn recommend_mode_keeps_control_permission_tool_request_activity() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            approval_mode: CoshApprovalMode::Recommend,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[governed(AgentEvent::ToolPermissionRequest {
                run_id: "run-1".to_string(),
                request_id: "ctrl-write".to_string(),
                tool_name: "Write".to_string(),
                tool_input: serde_json::json!({
                    "file_path": "/tmp/cosh-write.txt",
                    "content": "ok"
                }),
                tool_use_id: "toolu-write".to_string(),
            })],
        );

        let mut output = Vec::new();
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(
            output.contains("Write requested: /tmp/cosh-write.txt (new file); [Details]"),
            "{output}"
        );
    }

    #[test]
    fn control_protocol_policy_suppresses_known_foreground_shell_echo() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        state
            .control
            .mark_provider_shell_transcript_seen("toolu-shell");
        let ids = record_activity_rows_with_policy(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-shell".to_string()),
                    name: "run_shell_command".to_string(),
                    input: r#"{"command":"df -h"}"#.to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-shell".to_string(),
                    stream: "stdout".to_string(),
                    text: "Filesystem\n/dev/disk1\n".to_string(),
                }),
                governed(AgentEvent::ToolCompleted {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-shell".to_string(),
                    status: "success".to_string(),
                }),
            ],
            ActivityRecordPolicy {
                suppress_provider_native_shell: true,
            },
        );

        assert!(ids.is_empty(), "{ids:?}");
        assert!(state.activity.rows.is_empty(), "{:?}", state.activity.rows);
    }

    #[test]
    fn control_permission_shell_output_is_not_rendered_as_provider_native_transcript() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolPermissionRequest {
                    run_id: "run-1".to_string(),
                    request_id: "ctrl-1".to_string(),
                    tool_name: "run_shell_command".to_string(),
                    tool_input: serde_json::json!({ "command": "ssh -V" }),
                    tool_use_id: "toolu-shell".to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-shell".to_string(),
                    stream: "stdout".to_string(),
                    text: "PROVIDER OUTPUT SHOULD NOT RENDER\n".to_string(),
                }),
            ],
        );

        let mut output = Vec::new();
        render_provider_native_shell_transcript(&mut state, &ids, &mut output)
            .expect("render shell transcript");
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(!output.contains("$ ssh -V"), "{output}");
        assert!(
            !output.contains("PROVIDER OUTPUT SHOULD NOT RENDER"),
            "{output}"
        );
    }

    #[test]
    fn provider_native_streamed_shell_output_uses_tool_id_not_pending_order() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("tool-first".to_string()),
                    name: "run_shell_command".to_string(),
                    input: r#"{"command":"echo FIRST"}"#.to_string(),
                }),
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("tool-second".to_string()),
                    name: "run_shell_command".to_string(),
                    input: r#"{"command":"echo SECOND"}"#.to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "tool-second".to_string(),
                    stream: "stdout".to_string(),
                    text: "SECOND\n".to_string(),
                }),
            ],
        );

        let mut output = Vec::new();
        render_provider_native_shell_transcript(&mut state, &ids, &mut output)
            .expect("render shell transcript");
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("$ echo SECOND\nSECOND\n"), "{output}");
        assert!(!output.contains("$ echo FIRST\nSECOND"), "{output}");
        assert!(!output.contains("Activity"), "{output}");
    }

    #[test]
    fn provider_native_shell_error_completion_uses_transcript_not_activity() {
        let mut state = InlineState {
            language: cosh_shell::Language::EnUs,
            ..InlineState::default()
        };
        let ids = record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("toolu-1".to_string()),
                    name: "run_shell_command".to_string(),
                    input: serde_json::json!({ "command": "df -h" }).to_string(),
                }),
                governed(AgentEvent::ToolCompleted {
                    run_id: "run-1".to_string(),
                    tool_id: "toolu-1".to_string(),
                    status: "error".to_string(),
                }),
            ],
        );

        let mut output = Vec::new();
        render_provider_native_shell_transcript(&mut state, &ids, &mut output)
            .expect("render shell transcript");
        render_activity_rows(&state, &ids, &mut output).expect("render activity");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("$ df -h"), "{output}");
        assert!(output.contains("tool status: error"), "{output}");
        assert!(!output.contains("Activity"), "{output}");
        assert!(!output.contains("Tool error"), "{output}");
    }

    #[test]
    fn tool_output_ref_uses_private_permissions() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-activity-output-ref-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let path = write_tool_output_ref(&dir, "out-1", "secret-ish\n").expect("write output ref");

        assert_eq!(
            std::fs::metadata(&dir)
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_handoff_activity_marks_user_interrupt_status() {
        let mut state = InlineState::default();
        let request = cosh_shell::types::ShellHandoffRequest::new(
            "sleep 100",
            "$ sleep 100",
            "approved_provider_shell_tool",
            "user",
            "req-1",
            "run-1",
            0,
        )
        .expect("handoff request");
        state
            .control
            .shell_handoff_mut()
            .enqueue_approved_request(request);
        state
            .control
            .shell_handoff_mut()
            .emit_next_approved()
            .expect("emit pending handoff");
        let block = CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "session-1".to_string(),
            command: "sleep 100".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 1,
            ended_at_ms: 10,
            duration_ms: 9,
            exit_code: 130,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: Some("/tmp/internal-output-ref.txt".to_string()),
                terminal_output_bytes: 0,
            },
        };

        let ids = record_approved_shell_handoff_blocks(&mut state, &[block]);

        assert_eq!(ids, vec!["handoff-1"]);
        let row = state
            .activity
            .rows
            .iter()
            .find(|row| row.id == "handoff-1")
            .expect("handoff row");
        assert_eq!(row.status, "interrupted");
        assert!(row.detail.contains("status: interrupted"), "{}", row.detail);
        assert!(row.detail.contains("exit_code: 130"), "{}", row.detail);
        assert!(
            row.detail
                .contains("output_id: terminal-output://session-1/cmd-1"),
            "{}",
            row.detail
        );
    }

    #[test]
    fn activity_skill_summary_uses_state_language() {
        let mut state = InlineState {
            language: cosh_shell::Language::ZhCn,
            ..InlineState::default()
        };
        record_activity_rows(
            &mut state,
            &[governed(AgentEvent::SkillLoadFailed {
                run_id: "run-1".to_string(),
                skill: "memory".to_string(),
                error: "missing file".to_string(),
            })],
        );

        let row = state
            .activity
            .rows
            .iter()
            .find(|row| row.id == "skill-1")
            .expect("activity row");
        assert_eq!(row.summary, "memory 失败");
        assert!(row.detail.contains("status: failed"));
    }

    #[test]
    fn activity_interactive_handoff_summary_uses_state_language() {
        let mut state = InlineState {
            language: cosh_shell::Language::ZhCn,
            ..InlineState::default()
        };
        record_activity_rows(
            &mut state,
            &[
                governed(AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("tool-use-1".to_string()),
                    name: "Bash".to_string(),
                    input: serde_json::json!({ "command": "sudo systemctl status sshd" })
                        .to_string(),
                }),
                governed(AgentEvent::ToolOutputDelta {
                    run_id: "run-1".to_string(),
                    tool_id: "tool-use-1".to_string(),
                    stream: "stderr".to_string(),
                    text: "sudo: a terminal is required\n".to_string(),
                }),
                governed(AgentEvent::ToolCompleted {
                    run_id: "run-1".to_string(),
                    tool_id: "tool-use-1".to_string(),
                    status: "error".to_string(),
                }),
            ],
        );

        let row = state
            .activity
            .rows
            .iter()
            .find(|row| row.id == "tool-2")
            .expect("activity row");
        assert_eq!(
            row.summary,
            "sudo: a terminal is required; 可能需要前台 shell；[Send to shell] handoff-1；[Details] tool-2"
        );
        assert!(row
            .detail
            .contains("interactive_hint: may_require_foreground_shell"));
    }
}
