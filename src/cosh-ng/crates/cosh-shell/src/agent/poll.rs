use std::time::Duration;

use cosh_shell::adapter::AgentRunPoll;
use cosh_shell::tools::is_shell_tool_name;

use crate::agent::continuation::{
    render_fresh_turn_recovery_notice, shell_handoff_first_text_fallback_request,
};
use crate::agent::events::{
    active_run_has_unrendered_interaction, render_active_agent_event,
    render_new_agent_structured_events, state_has_pending_interaction,
};
use crate::agent::finish::finish_active_agent_run;
use crate::agent::heartbeat::render_agent_heartbeat;
use crate::agent::run::{has_queued_run_before_held_text, start_agent_run, ActiveAgentRun};
use crate::approval::broker::{provider_deny_response, ProviderResponseInput};
use crate::runtime::evidence_delivery::stalled_provider_shell_handoff_continuation_request;
use crate::runtime::prelude::*;

pub(crate) fn poll_active_agent_run<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    poll_active_agent_run_with_policy(state, output, adapter, false, true, true, false)
}

pub(crate) fn poll_active_agent_run_deferred<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    if let Some(active_run) = state.agent_run.active.as_mut() {
        active_run.status_animation.clear(output)?;
        output.flush()?;
    }
    poll_active_agent_run_with_policy(state, output, adapter, true, false, false, true)
}

fn poll_active_agent_run_with_policy<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
    force_hold_output: bool,
    render_structured: bool,
    finish_completed: bool,
    suppress_heartbeat: bool,
) -> std::io::Result<()> {
    let mut should_finish = false;
    let mut first_text_fallback: Option<(AgentRequest, Option<usize>)> = None;
    loop {
        let pending_interaction_before_poll = state_has_pending_interaction(state);
        let queued_before_held_text = has_queued_run_before_held_text(state);
        let shell_handoff_in_progress = state.control.shell_handoff().has_active_handoff();
        let deny_shell_after_foreground_evidence =
            state.evidence.has_open_provider_shell_evidence()
                || state.agent_run.host_executed_shell_result_delivered;
        let stalled_provider_shell_fallback = state
            .agent_run
            .active
            .as_ref()
            .is_some_and(active_run_has_stalled_shell_evidence_delivery)
            .then(|| stalled_provider_shell_handoff_continuation_request(state))
            .flatten();
        let Some(active_run) = state.agent_run.active.as_mut() else {
            return Ok(());
        };
        if active_run.completed {
            active_run.status_animation.clear(output)?;
            should_finish = finish_completed;
            break;
        }

        let event = match active_run
            .handle
            .poll_event_timeout(Duration::from_millis(0))
        {
            Ok(AgentRunPoll::Event(event)) => event,
            Ok(AgentRunPoll::Timeout) => {
                if let Some(fallback) = stalled_provider_shell_fallback {
                    first_text_fallback = Some((fallback, active_run.selectable_after_event_index));
                    break;
                }
                if let Some(fallback) = shell_handoff_first_text_fallback_request(active_run) {
                    first_text_fallback = Some((fallback, active_run.selectable_after_event_index));
                    break;
                }
                if pending_interaction_before_poll
                    || queued_before_held_text
                    || active_run_has_unrendered_interaction(active_run)
                {
                    active_run.status_animation.clear(output)?;
                    output.flush()?;
                    break;
                }
                render_agent_heartbeat(
                    active_run,
                    output,
                    suppress_heartbeat || shell_handoff_in_progress,
                )?;
                output.flush()?;
                break;
            }
            Ok(AgentRunPoll::Finished) => {
                should_finish = true;
                break;
            }
            Err(err) => AgentEvent::AgentFailed {
                run_id: active_run.request.id.clone(),
                error: err.message,
            },
        };

        let terminal_event = matches!(
            event,
            AgentEvent::AgentCompleted { .. }
                | AgentEvent::AgentFailed { .. }
                | AgentEvent::AgentCancelled { .. }
        );
        let deny_reentrant_shell_request = deny_shell_after_foreground_evidence
            || event_run_id(&event)
                .is_some_and(|run_id| state.control.provider_shell_handoff_run_seen(run_id));
        deny_reentrant_shell_request_after_foreground_evidence(
            active_run,
            &event,
            deny_reentrant_shell_request,
        );
        let provider_progress_observed = shell_evidence_provider_progress_observed(&event);
        let hold_stable_text = pending_interaction_before_poll
            || queued_before_held_text
            || active_run_has_unrendered_interaction(active_run)
            || force_hold_output;
        render_active_agent_event(active_run, event, output, hold_stable_text)?;
        if provider_progress_observed {
            state
                .evidence
                .mark_provider_progress_observed(terminal_event);
        }
        output.flush()?;
        if terminal_event {
            active_run.status_animation.clear(output)?;
            active_run.completed = true;
            should_finish = finish_completed;
            break;
        }
    }

    if let Some((fallback, selectable_after_event_index)) = first_text_fallback {
        if let Some(mut active_run) = state.agent_run.active.take() {
            active_run.handle.cancel();
            active_run.status_animation.clear(output)?;
        }
        render_fresh_turn_recovery_notice(state, output)?;
        start_agent_run(
            &fallback,
            adapter,
            state,
            output,
            selectable_after_event_index,
        )?;
        return Ok(());
    }

    if render_structured {
        render_new_agent_structured_events(state, output, adapter)?;
        output.flush()?;
    }

    if should_finish {
        finish_active_agent_run(state, output, adapter)?;
    }

    Ok(())
}

fn active_run_has_stalled_shell_evidence_delivery(active_run: &ActiveAgentRun) -> bool {
    !active_run.has_visible_text_delta && active_run.started_at.elapsed() >= Duration::from_secs(15)
}

fn event_run_id(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::TextDelta { run_id, .. }
        | AgentEvent::StatusChanged { run_id, .. }
        | AgentEvent::SkillLoadStarted { run_id, .. }
        | AgentEvent::SkillLoadCompleted { run_id, .. }
        | AgentEvent::SkillLoadFailed { run_id, .. }
        | AgentEvent::ToolCall { run_id, .. }
        | AgentEvent::ToolOutputDelta { run_id, .. }
        | AgentEvent::ToolPermissionRequest { run_id, .. }
        | AgentEvent::ToolCompleted { run_id, .. }
        | AgentEvent::UserQuestion { run_id, .. }
        | AgentEvent::Action { run_id, .. }
        | AgentEvent::AgentCompleted { run_id, .. }
        | AgentEvent::AgentFailed { run_id, .. }
        | AgentEvent::AgentCancelled { run_id, .. }
        | AgentEvent::Recommendation { run_id, .. } => Some(run_id),
    }
}

fn deny_reentrant_shell_request_after_foreground_evidence(
    active_run: &ActiveAgentRun,
    event: &AgentEvent,
    deny_shell_after_foreground_evidence: bool,
) {
    if !deny_shell_after_foreground_evidence {
        return;
    }
    let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_input,
        tool_use_id,
        ..
    } = event
    else {
        return;
    };
    if !is_shell_tool_name(tool_name) {
        return;
    }
    let _ = active_run.handle.respond_approval(provider_deny_response(
        ProviderResponseInput {
            request_id,
            tool_use_id: Some(tool_use_id),
            tool_input: Some(tool_input),
        },
        "The foreground shell command already completed and its output was injected. Summarize the existing shell evidence or ask the user to start a new request before running another shell command.".to_string(),
    ));
}

fn shell_evidence_provider_progress_observed(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TextDelta { .. }
            | AgentEvent::ToolPermissionRequest { .. }
            | AgentEvent::AgentCompleted { .. }
            | AgentEvent::AgentFailed { .. }
            | AgentEvent::AgentCancelled { .. }
    )
}
