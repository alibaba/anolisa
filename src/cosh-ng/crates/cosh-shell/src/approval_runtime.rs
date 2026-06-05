use super::*;
use cosh_shell::agent_render::approval_action_at;

pub(super) fn record_approval_requests(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    run_request: Option<&AgentRequest>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let session_id = run_request
        .map(|request| request.session_id.clone())
        .unwrap_or_else(|| "unknown-session".to_string());
    let cwd = run_request
        .map(|request| request.command_block.cwd.clone())
        .unwrap_or_else(|| "<unknown>".to_string());
    for event in governed_events {
        let request = approval_request_from_event(state, event, &session_id, &cwd);

        if let Some(request) = request {
            if state.approval_requests.iter().any(|existing| {
                existing.run_id == request.run_id
                    && existing.kind == request.kind
                    && existing.subject == request.subject
                    && existing.preview == request.preview
            }) {
                continue;
            }
            ids.push(request.id.clone());
            state.approval_requests.push(request);
        }
    }
    ids
}

pub(super) fn approval_request_from_governed_event(
    state: &InlineState,
    event: &GovernedEvent,
    run_request: Option<&AgentRequest>,
) -> Option<RuntimeApprovalRequest> {
    let session_id = run_request
        .map(|request| request.session_id.clone())
        .unwrap_or_else(|| "unknown-session".to_string());
    let cwd = run_request
        .map(|request| request.command_block.cwd.clone())
        .unwrap_or_else(|| "<unknown>".to_string());
    approval_request_from_event(state, event, &session_id, &cwd)
}

fn approval_request_from_event(
    state: &InlineState,
    event: &GovernedEvent,
    session_id: &str,
    cwd: &str,
) -> Option<RuntimeApprovalRequest> {
    match &event.event {
        AgentEvent::ToolCall {
            run_id,
            name,
            input,
        } => Some(RuntimeApprovalRequest {
            id: next_approval_id(state),
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            source: "agent",
            kind: ApprovalRequestKind::Tool,
            subject: format!("tool {name}"),
            preview: input.clone(),
            risk: "medium",
            status: ApprovalRequestStatus::Pending,
        }),
        AgentEvent::Action { run_id, command } => Some(RuntimeApprovalRequest {
            id: next_approval_id(state),
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            source: "agent",
            kind: ApprovalRequestKind::ShellCommand,
            subject: "shell command".to_string(),
            preview: command.clone(),
            risk: risk_for_command(command),
            status: ApprovalRequestStatus::Pending,
        }),
        _ => None,
    }
}

pub(super) fn record_auto_approved_request(
    state: &mut InlineState,
    mut request: RuntimeApprovalRequest,
) -> RuntimeApprovalRequest {
    request.status = ApprovalRequestStatus::Approved;
    state.approval_requests.push(request.clone());
    state
        .approval_journal
        .push(approval_journal_entry(&request));
    request
}

fn next_approval_id(state: &InlineState) -> String {
    format!("req-{}", state.approval_requests.len() + 1)
}

fn risk_for_command(command: &str) -> &'static str {
    if command.contains("sudo")
        || command.contains("rm ")
        || command.contains(">")
        || command.contains('|')
        || command.contains('$')
        || command.contains('`')
    {
        "high"
    } else {
        "medium"
    }
}

pub(super) fn render_approval_requests<W: Write>(
    state: &mut InlineState,
    approval_ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    if approval_ids.is_empty() {
        return Ok(());
    }

    render_current_approval_request(state, output)
}

fn render_current_approval_request<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let Some((index, request)) = state
        .approval_requests
        .iter()
        .enumerate()
        .find(|(_, request)| request.status == ApprovalRequestStatus::Pending)
    else {
        return Ok(());
    };

    if state.active_approval_panel_id.as_deref() == Some(request.id.as_str()) {
        return Ok(());
    }

    let pending_total = state
        .approval_requests
        .iter()
        .filter(|request| request.status == ApprovalRequestStatus::Pending)
        .count();
    let pending_position = state
        .approval_requests
        .iter()
        .take(index + 1)
        .filter(|request| request.status == ApprovalRequestStatus::Pending)
        .count();
    let next_pending = state
        .approval_requests
        .iter()
        .skip(index + 1)
        .find(|request| request.status == ApprovalRequestStatus::Pending);
    let preview_label = match request.kind {
        ApprovalRequestKind::Tool => "Tool input",
        ApprovalRequestKind::ShellCommand => "Command",
    };
    let next_label = next_pending.map(|next| format!("{} {}", next.id, next.subject));
    let selected_action = state
        .approval_focus
        .get(&request.id)
        .copied()
        .unwrap_or(ApprovalPanelAction::Approve);
    let expanded = state.expanded_approval_cards.contains(&request.id);
    let height = RatatuiInlineRenderer::for_terminal().write_approval_panel(
        output,
        ApprovalPanelModel {
            id: &request.id,
            kind: request.kind.label(),
            risk: request.risk,
            subject: &request.subject,
            preview_label,
            preview: &request.preview,
            queue_position: pending_position,
            queue_total: pending_total,
            next_label: next_label.as_deref(),
            selected_action,
            expanded,
        },
    )?;
    state.active_approval_panel_id = Some(request.id.clone());
    state.active_approval_panel_height = height;
    Ok(())
}

fn redraw_current_approval_request<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_approval_panel(state, output)?;
    render_current_approval_request(state, output)
}

fn clear_active_approval_panel<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let height = state.active_approval_panel_height;
    if height == 0 {
        state.active_approval_panel_id = None;
        return Ok(());
    }

    write!(output, "\x1b[{height}A")?;
    for row in 0..height {
        write!(output, "\r\x1b[2K")?;
        if row + 1 < height {
            write!(output, "\x1b[1B")?;
        }
    }
    if height > 1 {
        write!(output, "\x1b[{}A", height - 1)?;
    }
    write!(output, "\r")?;
    state.active_approval_panel_id = None;
    state.active_approval_panel_height = 0;
    Ok(())
}

pub(super) fn render_approval_actions<W: Write>(
    events: &[ShellEvent],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        if let Some((id, action)) = approval_focus_from_event(event) {
            let key = format!("approval-focus-{idx}");
            if !state.handled_approval_actions.insert(key) {
                continue;
            }
            if approval_is_pending(state, &id) {
                state.approval_focus.insert(id, action);
                redraw_current_approval_request(state, output)?;
                output.flush()?;
            }
            continue;
        }

        let Some(command) = approval_command_from_event(event) else {
            continue;
        };

        let key = format!("approval-{idx}");
        if !state.handled_approval_actions.insert(key) {
            continue;
        }

        if command.kind == ApprovalCommandKind::Details {
            if event.component.as_deref() == Some("card") {
                state
                    .approval_focus
                    .insert(command.id.clone(), ApprovalPanelAction::Details);
                state.expanded_approval_cards.insert(command.id.clone());
                redraw_current_approval_request(state, output)?;
            } else {
                render_runtime_details(state, &command.id, output)?;
            }
            output.flush()?;
            continue;
        }

        let Some(request_index) = state
            .approval_requests
            .iter()
            .position(|request| request.id == command.id)
        else {
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Approval not found",
                vec![format!(
                    "{} is not available; the approval card may already be resolved",
                    command.id
                )],
                None,
            )?;
            output.flush()?;
            continue;
        };

        if state.approval_requests[request_index].status != ApprovalRequestStatus::Pending {
            continue;
        }

        if let Some(decision) = apply_approval_decision(state, request_index, command.kind) {
            render_approval_resolution(state, &decision.request, decision.title, output)?;
            if decision.run_approved_tool {
                stop_active_agent_run_without_rendering(state, output)?;
                render_approved_tool_result(state, &decision.request, adapter, output)?;
            } else if should_send_approval_resolution_to_agent(state, &decision.request) {
                stop_active_agent_run_without_rendering(state, output)?;
                let request = approval_resolution_agent_request(&decision.request);
                start_agent_run(&request, adapter, state, output, Some(idx))?;
            }
            render_current_approval_request(state, output)?;
        }
        output.flush()?;
    }

    Ok(())
}

struct AppliedApprovalDecision {
    request: RuntimeApprovalRequest,
    title: &'static str,
    run_approved_tool: bool,
}

fn apply_approval_decision(
    state: &mut InlineState,
    request_index: usize,
    kind: ApprovalCommandKind,
) -> Option<AppliedApprovalDecision> {
    let (status, title) = match kind {
        ApprovalCommandKind::Approve => (ApprovalRequestStatus::Approved, "Approved"),
        ApprovalCommandKind::Deny => (ApprovalRequestStatus::Denied, "Denied"),
        ApprovalCommandKind::Cancel => (ApprovalRequestStatus::Cancelled, "Cancelled"),
        ApprovalCommandKind::Details => return None,
    };

    state.approval_requests[request_index].status = status;
    let request = state.approval_requests[request_index].clone();
    state
        .approval_journal
        .push(approval_journal_entry(&request));
    let run_approved_tool =
        status == ApprovalRequestStatus::Approved && request_is_executable_bash_tool(&request);

    Some(AppliedApprovalDecision {
        request,
        title,
        run_approved_tool,
    })
}

fn should_send_approval_resolution_to_agent(
    state: &InlineState,
    request: &RuntimeApprovalRequest,
) -> bool {
    matches!(
        request.status,
        ApprovalRequestStatus::Denied | ApprovalRequestStatus::Cancelled
    ) && !state
        .approval_requests
        .iter()
        .any(|request| request.status == ApprovalRequestStatus::Pending)
}

fn approval_resolution_agent_request(request: &RuntimeApprovalRequest) -> AgentRequest {
    let decision = match request.status {
        ApprovalRequestStatus::Denied => "denied by user",
        ApprovalRequestStatus::Cancelled => "cancelled by user",
        ApprovalRequestStatus::Pending => "pending",
        ApprovalRequestStatus::Approved => "approved",
    };
    let block_id = format!("approval-resolution-{}", request.id);
    let user_input = format!(
        "Approval result for request {id}\n\
         Tool: {subject}\n\
         Command: {command}\n\
         Decision: {decision}\n\
         No command ran.\n\
         Continue the same Agent session using this approval result. Do not claim the command executed. Provide a safe next step or ask for another approval if more evidence is required.",
        id = request.id,
        subject = request.subject,
        command = request.preview,
        decision = decision,
    );

    AgentRequest {
        id: format!("agent-request-{block_id}"),
        session_id: request.session_id.clone(),
        command_block: CommandBlock {
            id: block_id,
            session_id: request.session_id.clone(),
            command: user_input.clone(),
            cwd: request.cwd.clone(),
            end_cwd: request.cwd.clone(),
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        },
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some(user_input),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
    }
}

fn approval_is_pending(state: &InlineState, id: &str) -> bool {
    state
        .approval_requests
        .iter()
        .any(|request| request.id == id && request.status == ApprovalRequestStatus::Pending)
}

fn approval_focus_from_event(event: &ShellEvent) -> Option<(String, ApprovalPanelAction)> {
    if event.kind != ShellEventKind::UserInputIntercepted
        || event.component.as_deref() != Some("card")
        || event.message.as_deref() != Some("focus")
    {
        return None;
    }

    let (id, selected) = event.input.as_deref()?.split_once(':')?;
    let index = selected.trim().parse::<usize>().ok()?;
    let action = approval_action_at(index)?;
    Some((id.trim().to_string(), action))
}

fn approval_journal_entry(request: &RuntimeApprovalRequest) -> RuntimeApprovalJournalEntry {
    RuntimeApprovalJournalEntry {
        id: request.id.clone(),
        run_id: request.run_id.clone(),
        source: request.source,
        kind: request.kind,
        subject: request.subject.clone(),
        preview: request.preview.clone(),
        risk: request.risk,
        decision: request.status,
    }
}

pub(super) fn render_approval_journal<W: Write>(
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let entries = state
        .approval_journal
        .iter()
        .map(|entry| ApprovalJournalEntryModel {
            id: &entry.id,
            run_id: &entry.run_id,
            source: entry.source,
            decision: entry.decision.label(),
            kind: entry.kind.label(),
            risk: entry.risk,
            subject: &entry.subject,
            preview: &entry.preview,
        })
        .collect::<Vec<_>>();
    RatatuiInlineRenderer::for_terminal()
        .write_approval_journal_panel(output, ApprovalJournalPanelModel { entries: &entries })?;
    Ok(())
}

pub(super) fn render_approval_resolution<W: Write>(
    state: &mut InlineState,
    request: &RuntimeApprovalRequest,
    title: &str,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_approval_panel(state, output)?;
    let decision = match request.status {
        ApprovalRequestStatus::Pending => "pending",
        ApprovalRequestStatus::Approved => {
            if request.kind == ApprovalRequestKind::Tool {
                "approved"
            } else {
                "approved for display only"
            }
        }
        ApprovalRequestStatus::Denied => "denied",
        ApprovalRequestStatus::Cancelled => "cancelled by user",
    };

    let executable_bash = request.status == ApprovalRequestStatus::Approved
        && request_is_executable_bash_tool(request);
    let message = "";

    let kind = if request_is_executable_bash_tool(request) {
        "Bash tool"
    } else {
        request.kind.label()
    };

    RatatuiInlineRenderer::for_terminal().write_approval_receipt_panel(
        output,
        ApprovalReceiptPanelModel {
            title,
            id: &request.id,
            kind: if executable_bash { "" } else { kind },
            decision: if executable_bash { "" } else { decision },
            subject: &request.subject,
            preview: if executable_bash {
                ""
            } else {
                &request.preview
            },
            message,
        },
    )?;
    Ok(())
}

pub(super) fn render_approval_details<W: Write>(
    request: &RuntimeApprovalRequest,
    output: &mut W,
) -> std::io::Result<()> {
    let preview_label = match request.kind {
        ApprovalRequestKind::Tool => "Tool input",
        ApprovalRequestKind::ShellCommand => "Command",
    };

    RatatuiInlineRenderer::for_terminal().write_approval_details_panel(
        output,
        ApprovalDetailsPanelModel {
            id: &request.id,
            run_id: &request.run_id,
            source: request.source,
            kind: request.kind.label(),
            status: request.status.label(),
            risk: request.risk,
            subject: &request.subject,
            preview_label,
            preview: &request.preview,
        },
    )?;
    Ok(())
}
