use std::time::{Duration, Instant};

use cosh_shell::{
    agent_render::{AgentStatusAnimation, MarkdownStreamBlock},
    AgentRunHandle, AgentRunPoll,
};

use super::*;

pub(super) struct ActiveAgentRun {
    pub(super) request: AgentRequest,
    pub(super) handle: AgentRunHandle,
    pub(super) renderer: RatatuiInlineRenderer,
    pub(super) status_animation: AgentStatusAnimation,
    pub(super) markdown_stream: MarkdownStreamBlock,
    pub(super) governed_events: Vec<GovernedEvent>,
    pub(super) deferred_events: Vec<GovernedEvent>,
    pub(super) held_events: Vec<GovernedEvent>,
    pub(super) rendered_governed_event_count: usize,
    pub(super) selectable_after_event_index: Option<usize>,
    pub(super) started_at: Instant,
    pub(super) last_activity_at: Instant,
    pub(super) last_heartbeat_at: Instant,
    pub(super) current_phase: String,
    pub(super) current_message: String,
    pub(super) completed: bool,
}

#[derive(Debug, Clone)]
pub(super) struct PendingAgentRequest {
    pub(super) request: cosh_shell::AgentRequest,
    pub(super) selectable_after_event_index: Option<usize>,
    pub(super) before_held_text: bool,
}

const AGENT_HEARTBEAT_AFTER: Duration = Duration::from_secs(6);
const AGENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub(super) fn start_agent_run<W: Write>(
    request: &cosh_shell::AgentRequest,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
) -> std::io::Result<()> {
    start_agent_run_with_queue_policy(
        request,
        adapter,
        state,
        output,
        selectable_after_event_index,
        false,
    )
}

pub(super) fn start_agent_run_before_held_text<W: Write>(
    request: &cosh_shell::AgentRequest,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
) -> std::io::Result<()> {
    start_agent_run_with_queue_policy(
        request,
        adapter,
        state,
        output,
        selectable_after_event_index,
        true,
    )
}

fn start_agent_run_with_queue_policy<W: Write>(
    request: &cosh_shell::AgentRequest,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
    before_held_text: bool,
) -> std::io::Result<()> {
    if state.active_run.is_some() {
        queue_agent_request(
            state,
            PendingAgentRequest {
                request: request.clone(),
                selectable_after_event_index,
                before_held_text,
            },
        );
        return Ok(());
    }

    let renderer = RatatuiInlineRenderer::for_terminal();
    let markdown_stream = renderer.stream_markdown_agent();
    let mut status_animation = renderer.status_animation();
    if status_animation.is_enabled() {
        status_animation.render(output, "Thinking...")?;
    } else {
        renderer.write_loading(output)?;
    }
    output.flush()?;

    let handle = adapter.start_cancellable(request.clone());
    let now = Instant::now();
    state.active_run = Some(ActiveAgentRun {
        request: request.clone(),
        handle,
        renderer,
        status_animation,
        markdown_stream,
        governed_events: Vec::new(),
        deferred_events: Vec::new(),
        held_events: Vec::new(),
        rendered_governed_event_count: 0,
        selectable_after_event_index,
        started_at: now,
        last_activity_at: now,
        last_heartbeat_at: now,
        current_phase: "starting".to_string(),
        current_message: "waiting for Agent backend".to_string(),
        completed: false,
    });
    poll_active_agent_run(state, output, adapter)
}

fn queue_agent_request(state: &mut InlineState, pending: PendingAgentRequest) {
    if !pending.before_held_text {
        state.queued_agent_requests.push_back(pending);
        return;
    }

    let insert_at = state
        .queued_agent_requests
        .iter()
        .position(|queued| !queued.before_held_text)
        .unwrap_or(state.queued_agent_requests.len());
    state.queued_agent_requests.insert(insert_at, pending);
}

pub(super) fn poll_active_agent_run<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    let mut should_finish = false;
    loop {
        let pending_interaction_before_poll = state_has_pending_interaction(state);
        let queued_before_held_text = has_queued_run_before_held_text(state);
        let Some(active_run) = state.active_run.as_mut() else {
            return Ok(());
        };

        let event = match active_run
            .handle
            .poll_event_timeout(Duration::from_millis(0))
            .map_err(adapter_error_to_io)?
        {
            AgentRunPoll::Event(event) => event,
            AgentRunPoll::Timeout => {
                if pending_interaction_before_poll
                    || queued_before_held_text
                    || active_run_has_unrendered_interaction(active_run)
                {
                    active_run.status_animation.clear(output)?;
                    output.flush()?;
                    break;
                }
                render_agent_heartbeat(active_run, output)?;
                output.flush()?;
                break;
            }
            AgentRunPoll::Finished => {
                should_finish = true;
                break;
            }
        };

        let terminal_event = matches!(
            event,
            AgentEvent::AgentCompleted { .. }
                | AgentEvent::AgentFailed { .. }
                | AgentEvent::AgentCancelled { .. }
        );
        let hold_stable_text = pending_interaction_before_poll
            || queued_before_held_text
            || active_run_has_unrendered_interaction(active_run);
        render_active_agent_event(active_run, event, output, hold_stable_text)?;
        output.flush()?;
        if terminal_event {
            active_run.completed = true;
            should_finish = true;
            break;
        }
    }

    render_new_agent_structured_events(state, output, adapter)?;
    output.flush()?;

    if should_finish {
        finish_active_agent_run(state, output, adapter)?;
    }

    Ok(())
}

fn render_new_agent_structured_events<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    let (events, run_request) = {
        let Some(active_run) = state.active_run.as_mut() else {
            return Ok(());
        };
        let start = active_run.rendered_governed_event_count;
        let end = active_run.governed_events.len();
        if start >= end {
            return Ok(());
        }
        let events = active_run.governed_events[start..end].to_vec();
        if events.iter().any(is_interaction_governed_event) {
            active_run.status_animation.clear(output)?;
            active_run.markdown_stream.finish(output, None)?;
        }
        active_run.rendered_governed_event_count = end;
        (events, active_run.request.clone())
    };

    render_agent_structured_events(state, &events, Some(&run_request), output, adapter)
}

fn render_agent_structured_events<W: Write>(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    run_request: Option<&AgentRequest>,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    let activity_ids = record_activity_rows(state, governed_events);
    render_activity_rows(state, &activity_ids, output)?;
    let question_ids = record_user_questions(state, governed_events);
    render_user_questions(state, &question_ids, output)?;
    if render_auto_approved_tool(state, governed_events, run_request, output, adapter)? {
        return Ok(());
    }
    let approval_ids = record_approval_requests(state, governed_events, run_request);
    render_approval_requests(state, &approval_ids, output)?;
    Ok(())
}

fn render_auto_approved_tool<W: Write>(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    run_request: Option<&AgentRequest>,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<bool> {
    if state.approval_mode != ApprovalMode::Auto {
        return Ok(false);
    }

    for event in governed_events {
        let Some(request) = approval_request_from_governed_event(state, event, run_request) else {
            continue;
        };
        if request_is_readonly_builtin_tool(&request) {
            let request = record_auto_approved_request(state, request);
            render_approval_resolution(state, &request, "Auto-approved", output)?;
            continue;
        }

        if !request_is_executable_bash_tool(&request)
            || can_run_approved_bash_tool(&request.preview).is_err()
        {
            continue;
        }

        let request = record_auto_approved_request(state, request);
        render_approval_resolution(state, &request, "Auto-approved", output)?;
        stop_active_agent_run_without_rendering(state, output)?;
        render_approved_tool_result(state, &request, adapter, output)?;
        return Ok(true);
    }

    Ok(false)
}

fn render_agent_heartbeat<W: Write>(
    active_run: &mut ActiveAgentRun,
    output: &mut W,
) -> std::io::Result<()> {
    let now = Instant::now();
    if active_run.status_animation.is_enabled() {
        return active_run.status_animation.render(output, "Thinking...");
    }

    if now.duration_since(active_run.started_at) < AGENT_HEARTBEAT_AFTER {
        return Ok(());
    }
    if now.duration_since(active_run.last_activity_at) < AGENT_HEARTBEAT_AFTER {
        return Ok(());
    }
    if now.duration_since(active_run.last_heartbeat_at) < AGENT_HEARTBEAT_INTERVAL {
        return Ok(());
    }

    active_run.last_heartbeat_at = now;
    let elapsed = now.duration_since(active_run.started_at).as_secs_f32();
    writeln!(output)?;
    active_run.renderer.write_notice(
        output,
        "Agent",
        vec![format!("Still working... {elapsed:.0}s")],
        None,
    )
}

fn render_active_agent_event<W: Write>(
    active_run: &mut ActiveAgentRun,
    event: AgentEvent,
    output: &mut W,
    hold_stable_text: bool,
) -> std::io::Result<()> {
    let governed = govern_agent_events(&[event], &Policy::default()).events;
    remember_agent_activity(active_run, &governed);
    if governed
        .first()
        .is_some_and(|event| matches!(event.event, AgentEvent::TextDelta { .. }))
    {
        if hold_stable_text {
            active_run.held_events.extend(governed.clone());
        } else {
            active_run.status_animation.clear(output)?;
            render_held_events_into_active_run(active_run, &governed, output)?;
        }
    } else if governed
        .first()
        .is_some_and(|event| matches!(event.event, AgentEvent::AgentCompleted { .. }))
    {
    } else {
        active_run.deferred_events.extend(
            governed
                .iter()
                .filter(|event| should_render_governance_block(event))
                .cloned(),
        );
    }
    active_run.governed_events.extend(governed);
    Ok(())
}

pub(super) fn state_has_pending_interaction(state: &InlineState) -> bool {
    has_pending_question(state)
        || state
            .approval_requests
            .iter()
            .any(|request| request.status == ApprovalRequestStatus::Pending)
}

fn active_run_has_unrendered_interaction(active_run: &ActiveAgentRun) -> bool {
    active_run.governed_events[active_run.rendered_governed_event_count..]
        .iter()
        .any(is_interaction_governed_event)
}

fn is_interaction_governed_event(event: &GovernedEvent) -> bool {
    matches!(
        event.event,
        AgentEvent::ToolCall { .. } | AgentEvent::UserQuestion { .. } | AgentEvent::Action { .. }
    )
}

fn render_held_events_into_active_run<W: Write>(
    active_run: &mut ActiveAgentRun,
    events: &[GovernedEvent],
    output: &mut W,
) -> std::io::Result<()> {
    for event in events {
        if matches!(event.event, AgentEvent::TextDelta { .. }) {
            active_run
                .markdown_stream
                .write_delta(output, &event.display_text)?;
        } else if should_render_governance_block(event) {
            active_run
                .renderer
                .write_governed_events(output, std::slice::from_ref(event))?;
        }
    }
    Ok(())
}

pub(super) fn flush_held_agent_events<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if state_has_pending_interaction(state) {
        return Ok(());
    }

    if let Some(active_run) = state.active_run.as_mut() {
        if active_run.held_events.is_empty() {
            return Ok(());
        }
        active_run.status_animation.clear(output)?;
        let held_events = std::mem::take(&mut active_run.held_events);
        render_held_events_into_active_run(active_run, &held_events, output)?;
        output.flush()?;
        return Ok(());
    }

    if state.held_agent_events.is_empty() {
        return Ok(());
    }
    let held_events = std::mem::take(&mut state.held_agent_events);
    let renderer = RatatuiInlineRenderer::for_terminal();
    let mut stream = renderer.stream_markdown_agent();
    for event in &held_events {
        if matches!(event.event, AgentEvent::TextDelta { .. }) {
            stream.write_delta(output, &event.display_text)?;
        } else if should_render_governance_block(event) {
            renderer.write_governed_events(output, std::slice::from_ref(event))?;
        }
    }
    stream.finish(output, None)?;
    output.flush()?;
    Ok(())
}

pub(super) fn stop_active_agent_run_without_rendering<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    state.held_agent_events.clear();
    let Some(mut active_run) = state.active_run.take() else {
        return Ok(());
    };

    active_run.handle.cancel();
    active_run.status_animation.clear(output)?;
    active_run.held_events.clear();
    active_run.deferred_events.clear();
    output.flush()?;
    Ok(())
}

fn remember_agent_activity(active_run: &mut ActiveAgentRun, governed: &[GovernedEvent]) {
    if governed.is_empty() {
        return;
    }

    let now = Instant::now();
    active_run.last_activity_at = now;
    for event in governed {
        match &event.event {
            AgentEvent::StatusChanged { phase, message, .. } => {
                active_run.current_phase = phase.clone();
                active_run.current_message = message.clone();
            }
            AgentEvent::TextDelta { .. } => {
                active_run.current_phase = "streaming".to_string();
                active_run.current_message = "receiving Agent response".to_string();
            }
            AgentEvent::SkillLoadStarted { skill, .. } => {
                active_run.current_phase = "skill".to_string();
                active_run.current_message = format!("loading skill {skill}");
            }
            AgentEvent::ToolCall { name, .. } => {
                active_run.current_phase = "approval".to_string();
                active_run.current_message = format!("waiting for approval: tool {name}");
            }
            AgentEvent::UserQuestion { question, .. } => {
                active_run.current_phase = "question".to_string();
                active_run.current_message = format!("waiting for user answer: {question}");
            }
            AgentEvent::Action { command, .. } => {
                active_run.current_phase = "approval".to_string();
                active_run.current_message = format!("waiting for approval: {command}");
            }
            AgentEvent::ToolOutputDelta { tool_id, .. } => {
                active_run.current_phase = "tool".to_string();
                active_run.current_message = format!("capturing output from {tool_id}");
            }
            AgentEvent::ToolCompleted {
                tool_id, status, ..
            } => {
                active_run.current_phase = "tool".to_string();
                active_run.current_message = format!("{tool_id} completed with status {status}");
            }
            AgentEvent::AgentCompleted { summary, .. } => {
                active_run.current_phase = "completed".to_string();
                active_run.current_message = summary.clone();
            }
            AgentEvent::AgentFailed { error, .. } => {
                active_run.current_phase = "failed".to_string();
                active_run.current_message = error.clone();
            }
            AgentEvent::AgentCancelled { reason, .. } => {
                active_run.current_phase = "cancelled".to_string();
                active_run.current_message = reason.clone();
            }
            AgentEvent::Recommendation { summary, .. }
            | AgentEvent::SkillLoadCompleted { summary, .. } => {
                active_run.current_message = summary.clone();
            }
            AgentEvent::SkillLoadFailed { skill, error, .. } => {
                active_run.current_phase = "skill".to_string();
                active_run.current_message = format!("{skill} failed: {error}");
            }
        }
    }
}

fn finish_active_agent_run<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    let Some(mut active_run) = state.active_run.take() else {
        return Ok(());
    };

    active_run.status_animation.clear(output)?;
    if !active_run.held_events.is_empty() {
        if state_has_pending_interaction(state) || has_queued_run_before_held_text(state) {
            state
                .held_agent_events
                .extend(active_run.held_events.drain(..));
        } else {
            let held_events = std::mem::take(&mut active_run.held_events);
            render_held_events_into_active_run(&mut active_run, &held_events, output)?;
        }
    }
    active_run.markdown_stream.finish(output, None)?;
    if !active_run.deferred_events.is_empty() {
        active_run
            .renderer
            .write_governed_events(output, &active_run.deferred_events)?;
    }

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
    render_selectable_recommendations(&active_run.governed_events, output)?;
    output.flush()?;

    if let Some(pending) = state.queued_agent_requests.pop_front() {
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

fn has_queued_run_before_held_text(state: &InlineState) -> bool {
    state
        .queued_agent_requests
        .iter()
        .any(|pending| pending.before_held_text)
}

fn should_render_governance_block(event: &GovernedEvent) -> bool {
    match &event.event {
        AgentEvent::StatusChanged { .. } => false,
        AgentEvent::Recommendation { .. } => false,
        AgentEvent::ToolCall { .. }
        | AgentEvent::UserQuestion { .. }
        | AgentEvent::Action { .. } => false,
        AgentEvent::AgentFailed { .. } | AgentEvent::AgentCancelled { .. } => true,
        AgentEvent::SkillLoadStarted { .. }
        | AgentEvent::SkillLoadCompleted { .. }
        | AgentEvent::SkillLoadFailed { .. }
        | AgentEvent::ToolOutputDelta { .. }
        | AgentEvent::ToolCompleted { .. } => false,
        AgentEvent::TextDelta { .. } | AgentEvent::AgentCompleted { .. } => false,
    }
}

fn adapter_error_to_io(err: cosh_shell::AdapterError) -> std::io::Error {
    std::io::Error::other(err.message)
}
