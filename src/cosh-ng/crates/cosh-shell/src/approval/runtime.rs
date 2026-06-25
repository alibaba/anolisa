use crate::agent::run::ActiveAgentRun;
use crate::approval::broker::ApprovalOutcome;
use crate::approval::cards::write_approval_receipt;
use crate::approval::handoff::{queue_approved_shell_handoff, queue_interactive_shell_handoff};
use crate::approval::panel::{
    approval_focus_from_event, approval_is_pending, clear_active_approval_panel,
    redraw_current_approval_request, render_current_approval_request,
};
use crate::approval::provider::{mark_provider_approval_resolved, provider_approval_response};
use crate::approval::resolution::{
    apply_approval_decision, approval_outcome_for_request, approval_resolution_agent_request,
    request_can_receive_host_executed_result, should_send_approval_resolution_to_agent,
};
use crate::runtime::details::agent_request_from_details_input;
use crate::runtime::prelude::*;

pub(crate) fn render_approval_actions<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        if let Some((id, action)) = approval_focus_from_event(event, &state.approvals.requests) {
            let key = format!("approval-focus-{event_index}");
            if !state.approvals.handled_actions.insert(key) {
                continue;
            }
            if approval_is_pending(state, &id) {
                state.approvals.focus.insert(id, action);
                redraw_current_approval_request(state, output)?;
                output.flush()?;
            }
            continue;
        }

        let Some(command) = approval_command_from_event(event) else {
            continue;
        };

        let key = format!("approval-{event_index}");
        if !state.approvals.handled_actions.insert(key) {
            continue;
        }

        if command.kind == ApprovalCommandKind::Details {
            if event.component.as_deref() == Some("card") {
                state
                    .approvals
                    .focus
                    .insert(command.id.clone(), ApprovalPanelAction::Details);
                state.approvals.expanded_cards.insert(command.id.clone());
                redraw_current_approval_request(state, output)?;
            } else {
                if let Some(input) = event.input.as_deref() {
                    if let Some(result) =
                        agent_request_from_details_input(blocks, input, event_index)
                    {
                        match result {
                            Ok(request) => {
                                state.agent_run.needs_prompt_after_run = event.cwd.is_none();
                                start_agent_run(
                                    &request,
                                    adapter,
                                    state,
                                    output,
                                    Some(event_index),
                                )?;
                            }
                            Err(message) => {
                                let i18n = state.i18n();
                                RatatuiInlineRenderer::for_terminal().write_notice_panel(
                                    output,
                                    NoticePanelModel {
                                        title: i18n.t(MessageId::RuntimeDetailsUnavailableTitle),
                                        body: vec![message],
                                        footer: None,
                                    },
                                )?;
                            }
                        }
                        output.flush()?;
                        continue;
                    }
                }
                render_runtime_details(state, blocks, &command.id, output)?;
            }
            output.flush()?;
            continue;
        }

        if command.kind == ApprovalCommandKind::SendToShell {
            queue_interactive_shell_handoff(state, &command.id, output)?;
            output.flush()?;
            continue;
        }

        let Some(request_index) = state
            .approvals
            .requests
            .iter()
            .position(|request| request.id == command.id)
        else {
            let i18n = state.i18n();
            RatatuiInlineRenderer::for_terminal().write_notice_panel(
                output,
                NoticePanelModel {
                    title: i18n.t(MessageId::ApprovalNotFoundTitle),
                    body: vec![i18n.format(
                        MessageId::ApprovalNotFoundBody,
                        &[("id", command.id.as_str())],
                    )],
                    footer: None,
                },
            )?;
            output.flush()?;
            continue;
        };

        if state.approvals.requests[request_index].status != ApprovalRequestStatus::Pending {
            continue;
        }

        if let Some(decision) = apply_approval_decision(state, request_index, command.kind) {
            if let Some(ref ctrl_request_id) = decision.request.request_id {
                let outcome = approval_outcome_for_request(state, &decision.request);
                if outcome == ApprovalOutcome::ProviderNativeShellFallback {
                    let response = provider_approval_response(&decision.request, ctrl_request_id);
                    if let Some(active_run) = state.agent_run.active.as_mut() {
                        respond_active_run_approval(active_run, response);
                    }
                    mark_provider_approval_resolved(state);
                    clear_active_approval_panel(state, output)?;
                    render_approval_resolution(state, &decision.request, decision.title, output)?;
                    render_current_approval_request(state, output)?;
                    flush_held_agent_events(state, output)?;
                    continue;
                }

                if outcome == ApprovalOutcome::ForegroundShellHandoff {
                    render_approval_resolution(state, &decision.request, decision.title, output)?;
                    if !request_can_receive_host_executed_result(state, &decision.request) {
                        stop_active_agent_run_without_rendering(state, output)?;
                    }
                    queue_approved_shell_handoff(state, &decision.request);
                    render_current_approval_request(state, output)?;
                    continue;
                }

                let response = provider_approval_response(&decision.request, ctrl_request_id);
                if let Some(active_run) = state.agent_run.active.as_mut() {
                    respond_active_run_approval(active_run, response);
                }
                if decision.request.status == ApprovalRequestStatus::Approved {
                    mark_provider_approval_resolved(state);
                }
                clear_active_approval_panel(state, output)?;
                render_approval_resolution(state, &decision.request, decision.title, output)?;
                render_current_approval_request(state, output)?;
                flush_held_agent_events(state, output)?;
            } else {
                render_approval_resolution(state, &decision.request, decision.title, output)?;
                if decision.run_approved_tool {
                    stop_active_agent_run_without_rendering(state, output)?;
                    queue_approved_shell_handoff(state, &decision.request);
                } else if should_send_approval_resolution_to_agent(state, &decision.request) {
                    stop_active_agent_run_without_rendering(state, output)?;
                    let request = approval_resolution_agent_request(&decision.request);
                    start_agent_run(&request, adapter, state, output, Some(event_index))?;
                }
                render_current_approval_request(state, output)?;
            }
        }
        output.flush()?;
    }

    Ok(())
}

fn respond_active_run_approval(
    active_run: &mut ActiveAgentRun,
    response: ApprovalResponse,
) -> bool {
    let responded = active_run.handle.respond_approval(response).is_ok();
    if responded {
        active_run.last_activity_at = std::time::Instant::now();
    }
    responded
}

pub(crate) fn render_approval_resolution<W: Write>(
    state: &mut InlineState,
    request: &RuntimeApprovalRequest,
    title: MessageId,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_approval_panel(state, output)?;
    write_approval_receipt(state.language, request, state.i18n().t(title), output)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
