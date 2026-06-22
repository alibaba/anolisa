use crate::agent::continuation::{
    render_fresh_turn_recovery_notice, shell_handoff_resume_fallback_request,
};
use crate::agent::events::{
    flush_cosh_request_filter_into_active_run, render_agent_structured_events,
    render_held_events_into_active_run, state_has_pending_interaction,
};
use crate::agent::run::{has_queued_run_before_held_text, start_agent_run, ActiveAgentRun};
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
    let resume_fallback = if provider_timed_out {
        shell_handoff_resume_fallback_request(&active_run)
    } else {
        None
    };
    if let Some(fallback) = resume_fallback {
        render_recovery_context_before_notice(state, &active_run, output, adapter)?;
        render_fresh_turn_recovery_notice(state, output)?;
        start_agent_run(
            &fallback,
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
        output,
        adapter,
    )?;
    record_selectable_recommendations(
        state,
        &active_run.governed_events,
        active_run.selectable_after_event_index,
    );
    render_selectable_recommendations(&active_run.governed_events, active_run.language, output)?;
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

    for request in evidence_requests.auto_requests {
        start_agent_run(&request, adapter, state, output, None)?;
    }

    if let Some(pending) = state.agent_run.queued_requests.pop_front() {
        start_agent_run(
            &pending.request,
            adapter,
            state,
            output,
            pending.selectable_after_event_index,
        )?;
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

fn trim_queued_requests_after_provider_timeout(state: &mut InlineState) -> usize {
    if state.agent_run.queued_requests.len() <= 1 {
        return 0;
    }
    let keep = state.agent_run.queued_requests.pop_front();
    let dropped = state.agent_run.queued_requests.len();
    state.agent_run.queued_requests.clear();
    if let Some(keep) = keep {
        state.agent_run.queued_requests.push_back(keep);
    }
    dropped
}
