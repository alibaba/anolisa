use crate::agent::continuation::{
    render_fresh_turn_recovery_notice, shell_handoff_resume_fallback_request,
};
use crate::agent::events::{
    flush_cosh_request_filter_into_active_run, render_agent_structured_events,
    render_held_events_into_active_run, state_has_pending_interaction,
};
use crate::agent::run::{
    has_queued_run_before_held_text, start_agent_run_with_origin, start_pending_agent_run,
    ActiveAgentRun, PendingRequestClass,
};
use crate::question::terminal::cleanup_question_for_terminal_owner;
use crate::recommendation::personal_integration::record_finished_agent_run;
use crate::runtime::evidence_requests::{
    record_cosh_requests_from_active_run, render_pending_evidence_requests,
};
use crate::runtime::prelude::*;

pub(crate) fn finish_active_agent_run<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    let Some(mut active_run) = state.agent_run.active.take() else {
        return Ok(());
    };

    cleanup_question_for_terminal_owner(state, output, &active_run.request.id)?;
    active_run.status_animation.clear(output)?;
    if !active_run.held_events.is_empty() {
        if state_has_pending_interaction(state) || has_queued_run_before_held_text(state) {
            state
                .agent_run
                .held_events
                .append(&mut active_run.held_events);
        } else {
            let held_events = std::mem::take(&mut active_run.held_events);
            render_held_events_into_active_run(&mut active_run, &held_events, output)?;
        }
    }
    flush_cosh_request_filter_into_active_run(&mut active_run, output)?;
    active_run.markdown_stream.finish(output, None)?;
    let provider_timed_out = active_run_provider_timed_out(&active_run);
    record_finished_agent_run(state, &active_run.request, &active_run.governed_events);
    let resume_fallback = if provider_timed_out {
        shell_handoff_resume_fallback_request(&active_run)
    } else {
        None
    };
    if let Some((fallback, origin)) = resume_fallback {
        render_recovery_context_before_notice(state, &active_run, output, adapter)?;
        render_fresh_turn_recovery_notice(state, output)?;
        // Provider-timeout resume is an internal fallback continuation.
        start_agent_run_with_origin(
            &fallback,
            origin,
            AgentStartIntent::InternalBestEffort,
            adapter,
            state,
            output,
            active_run.selectable_after_event_index,
        )?;
        return Ok(());
    }
    // Drain any unconsumed pending hook notifications into deferred_events
    // (orphan case: hook returned block, so no ToolPermissionRequest was emitted)
    for notification in active_run.pending_hook_notifications.drain(..) {
        active_run.deferred_events.push(GovernedEvent {
            decision: GovernanceDecision::Display,
            policy_decision: GovernancePolicyDecision::DisplayOnly,
            event: AgentEvent::HookNotification {
                run_id: active_run.request.id.clone(),
                hook_name: notification.hook_name,
                message: notification.message,
                tool_use_id: notification.tool_use_id,
                decision: notification.decision,
            },
            reason: "orphan hook notification".to_string(),
            display_text: String::new(),
            auto_execute: false,
        });
    }
    if !active_run.deferred_events.is_empty() {
        active_run
            .renderer
            .write_governed_events(output, &active_run.deferred_events)?;
    }
    let evidence_requests = record_cosh_requests_from_active_run(state, &mut active_run);
    for notice in &evidence_requests.notices {
        active_run.renderer.write_notice_panel(
            output,
            NoticePanelModel {
                title: "Evidence Request",
                body: vec![notice.clone()],
                footer: None,
            },
        )?;
    }
    render_pending_evidence_requests(state, &evidence_requests.card_ids, output)?;

    let remaining_structured_events =
        active_run.governed_events[active_run.rendered_governed_event_count..].to_vec();
    render_agent_structured_events(
        state,
        &remaining_structured_events,
        Some(&active_run.request),
        active_run.origin,
        output,
        adapter,
    )?;
    record_selectable_recommendations(
        state,
        &active_run.governed_events,
        active_run.origin,
        active_run.selectable_after_event_index,
    );
    render_selectable_recommendations(
        &active_run.governed_events,
        active_run.origin,
        active_run.language,
        output,
    )?;
    record_agent_run_facts(state, &active_run);
    state.auth.state = None;
    if provider_timed_out {
        let dropped = trim_queued_requests_after_provider_timeout(state);
        if dropped > 0 {
            active_run.renderer.write_notice_panel(
                output,
                NoticePanelModel {
                    title: state.i18n().t(MessageId::AgentStatusTitle),
                    body: vec![state.i18n().format(
                        MessageId::AgentProviderTimeoutDroppedQueuedBody,
                        &[("dropped", &dropped.to_string())],
                    )],
                    footer: None,
                },
            )?;
        }
    }
    output.flush()?;

    // A recommended automatic compaction has top priority at the idle
    // boundary: do not start any internal continuation or dequeue a run, since
    // that would keep `agent_run.active` set and postpone the compaction
    // indefinitely. Drop stale internal continuations (their captured context
    // is about to be rewritten by the compactor) and hold explicit user
    // requests in the queue so they resume in FIFO order after compaction.
    if state.control.session().compaction().has_pending_auto() {
        state
            .agent_run
            .queued_requests
            .retain(|pending| pending.intent == AgentStartIntent::UserInitiated);
        return Ok(());
    }

    for (request, origin) in evidence_requests.auto_requests {
        // Evidence auto-follow-ups are internal best-effort continuations; the
        // gate drops them while a compaction is pending or active.
        start_agent_run_with_origin(
            &request,
            origin,
            AgentStartIntent::InternalBestEffort,
            adapter,
            state,
            output,
            None,
        )?;
    }

    if let Some(pending) = state.agent_run.queued_requests.pop_front() {
        // Restart with the stored admission class so a control response that
        // gets re-queued (e.g. behind a fresh compaction) keeps its class.
        start_pending_agent_run(pending, adapter, state, output)?;
    }

    Ok(())
}

fn active_run_provider_timed_out(active_run: &ActiveAgentRun) -> bool {
    active_run
        .governed_events
        .iter()
        .any(governed_event_is_provider_timeout)
}

fn render_recovery_deferred_context<W: Write>(
    active_run: &ActiveAgentRun,
    output: &mut W,
) -> std::io::Result<()> {
    let events = active_run
        .deferred_events
        .iter()
        .filter(|event| !governed_event_is_provider_timeout(event))
        .cloned()
        .collect::<Vec<_>>();
    if events.is_empty() {
        return Ok(());
    }
    active_run.renderer.write_governed_events(output, &events)
}

fn render_recovery_context_before_notice<W: Write>(
    state: &mut InlineState,
    active_run: &ActiveAgentRun,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    render_recovery_deferred_context(active_run, output)?;
    let remaining_structured_events = active_run.governed_events
        [active_run.rendered_governed_event_count..]
        .iter()
        .filter(|event| !governed_event_is_provider_timeout(event))
        .cloned()
        .collect::<Vec<_>>();
    render_agent_structured_events(
        state,
        &remaining_structured_events,
        Some(&active_run.request),
        active_run.origin,
        output,
        adapter,
    )
}

fn governed_event_is_provider_timeout(event: &GovernedEvent) -> bool {
    matches!(
        &event.event,
        AgentEvent::AgentFailed { error, .. } if error.contains("Agent timed out:")
    )
}

/// Sheds queue backlog after a provider timeout without ever dropping a
/// control-protocol response.
///
/// Retained (in original FIFO order, so replay order stays deterministic):
/// - every [`PendingRequestClass::ControlResponse`] entry — its question or
///   approval state was already consumed and the user cannot re-issue it;
/// - the oldest normal request, so the user's next intent survives.
///
/// Every other normal request is dropped; the returned count covers exactly
/// those, which is what the user-visible notice reports.
fn trim_queued_requests_after_provider_timeout(state: &mut InlineState) -> usize {
    let before = state.agent_run.queued_requests.len();
    let mut kept_normal = false;
    state
        .agent_run
        .queued_requests
        .retain(|pending| match pending.class {
            PendingRequestClass::ControlResponse => true,
            PendingRequestClass::Normal => {
                if kept_normal {
                    false
                } else {
                    kept_normal = true;
                    true
                }
            }
        });
    before - state.agent_run.queued_requests.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::run::{PendingAgentRequest, PendingRequestClass};
    use crate::runtime::state::InlineState;
    use crate::types::{AgentMode, AgentRequest, CommandBlock, CommandStatus, OutputRefs};

    fn request(id: &str) -> AgentRequest {
        AgentRequest {
            id: id.to_string(),
            session_id: "shell-session".to_string(),
            command_block: CommandBlock {
                id: format!("cmd-{id}"),
                session_id: "shell-session".to_string(),
                command: "echo hi".to_string(),
                origin: Default::default(),
                cwd: "/repo".to_string(),
                end_cwd: "/repo".to_string(),
                started_at_ms: 1,
                ended_at_ms: 2,
                duration_ms: 1,
                exit_code: 0,
                status: CommandStatus::Completed,
                output: OutputRefs {
                    terminal_output_ref: None,
                    terminal_output_bytes: 0,
                },
                shell_environment_generation: None,
            },
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("queued".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }

    fn pending(id: &str, class: PendingRequestClass) -> PendingAgentRequest {
        PendingAgentRequest {
            request: request(id),
            origin: AgentRunOrigin::Standard,
            intent: AgentStartIntent::UserInitiated,
            class,
            selectable_after_event_index: None,
            before_held_text: false,
        }
    }

    #[test]
    fn provider_timeout_trim_keeps_control_responses_and_oldest_normal_in_order() {
        let mut state = InlineState::default();
        for (id, class) in [
            ("normal-a", PendingRequestClass::Normal),
            ("control-b", PendingRequestClass::ControlResponse),
            ("normal-c", PendingRequestClass::Normal),
            ("control-d", PendingRequestClass::ControlResponse),
            ("normal-e", PendingRequestClass::Normal),
        ] {
            state
                .agent_run
                .queued_requests
                .push_back(pending(id, class));
        }

        let dropped = trim_queued_requests_after_provider_timeout(&mut state);

        // Only the surplus normal requests were dropped and counted; every
        // control response survives, and FIFO order is untouched.
        assert_eq!(dropped, 2);
        let ids: Vec<&str> = state
            .agent_run
            .queued_requests
            .iter()
            .map(|pending| pending.request.id.as_str())
            .collect();
        assert_eq!(ids, ["normal-a", "control-b", "control-d"]);
    }

    #[test]
    fn provider_timeout_trim_drops_nothing_without_surplus_normals() {
        let mut state = InlineState::default();
        state
            .agent_run
            .queued_requests
            .push_back(pending("control-a", PendingRequestClass::ControlResponse));
        state
            .agent_run
            .queued_requests
            .push_back(pending("normal-b", PendingRequestClass::Normal));

        assert_eq!(trim_queued_requests_after_provider_timeout(&mut state), 0);
        assert_eq!(state.agent_run.queued_requests.len(), 2);
    }
}
