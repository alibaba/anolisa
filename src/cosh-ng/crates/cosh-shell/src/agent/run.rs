use std::time::Instant;

use cosh_shell::{
    adapter::AgentRunHandle,
    agent_render::{AgentStatusAnimation, MarkdownStreamBlock},
};

use crate::agent::continuation::provider_mode_for_agent_run;
use crate::agent::poll::poll_active_agent_run;
use crate::evidence::request::ParsedCoshRequest;
use crate::evidence::stream::{CoshRequestAuditRecord, CoshRequestStreamFilter};
use crate::runtime::prelude::*;

pub(crate) struct ActiveAgentRun {
    pub(crate) request: AgentRequest,
    pub(crate) handle: AgentRunHandle,
    pub(crate) provider_name: &'static str,
    pub(crate) language: cosh_shell::Language,
    pub(crate) renderer: RatatuiInlineRenderer,
    pub(crate) status_animation: AgentStatusAnimation,
    pub(crate) markdown_stream: MarkdownStreamBlock,
    pub(crate) governed_events: Vec<GovernedEvent>,
    pub(crate) deferred_events: Vec<GovernedEvent>,
    pub(crate) held_events: Vec<GovernedEvent>,
    pub(crate) cosh_request_filter: CoshRequestStreamFilter,
    pub(crate) pending_cosh_requests: Vec<ParsedCoshRequest>,
    pub(crate) pending_cosh_request_audits: Vec<CoshRequestAuditRecord>,
    pub(crate) rendered_governed_event_count: usize,
    pub(crate) selectable_after_event_index: Option<usize>,
    pub(crate) started_at: Instant,
    pub(crate) last_activity_at: Instant,
    pub(crate) last_heartbeat_at: Instant,
    pub(crate) current_phase: String,
    pub(crate) current_message: String,
    pub(crate) has_visible_text_delta: bool,
    pub(crate) completed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingAgentRequest {
    pub(crate) request: cosh_shell::types::AgentRequest,
    pub(crate) selectable_after_event_index: Option<usize>,
    pub(crate) before_held_text: bool,
}

pub(crate) fn start_agent_run<W: Write>(
    request: &cosh_shell::types::AgentRequest,
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

fn start_agent_run_with_queue_policy<W: Write>(
    request: &cosh_shell::types::AgentRequest,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
    before_held_text: bool,
) -> std::io::Result<()> {
    if state.agent_run.active.is_some() {
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

    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    let markdown_stream = renderer.stream_markdown_agent();
    let mut status_animation = renderer.status_animation();
    if status_animation.is_enabled() {
        status_animation.render(output, state.i18n().t(cosh_shell::MessageId::AgentThinking))?;
    } else {
        renderer
            .write_loading_text(output, state.i18n().t(cosh_shell::MessageId::AgentThinking))?;
    }
    output.flush()?;

    let mut request = request.clone();
    attach_continuity_prompt_hint(&mut request, state);
    let provider_mode = provider_mode_for_agent_run(&request, state.approval_mode);
    let handle = adapter.start_cancellable(request.clone(), provider_mode);
    let now = Instant::now();
    let i18n = state.i18n();
    state.agent_run.host_executed_shell_result_delivered = false;
    state.agent_run.active = Some(ActiveAgentRun {
        request,
        handle,
        provider_name: adapter.name(),
        language: state.language,
        renderer,
        status_animation,
        markdown_stream,
        governed_events: Vec::new(),
        deferred_events: Vec::new(),
        held_events: Vec::new(),
        cosh_request_filter: CoshRequestStreamFilter::default(),
        pending_cosh_requests: Vec::new(),
        pending_cosh_request_audits: Vec::new(),
        rendered_governed_event_count: 0,
        selectable_after_event_index,
        started_at: now,
        last_activity_at: now,
        last_heartbeat_at: now,
        current_phase: i18n
            .t(cosh_shell::MessageId::AgentStatusStarting)
            .to_string(),
        current_message: i18n
            .t(cosh_shell::MessageId::AgentStatusWaitingBackend)
            .to_string(),
        has_visible_text_delta: false,
        completed: false,
    });
    poll_active_agent_run(state, output, adapter)
}

fn queue_agent_request(state: &mut InlineState, pending: PendingAgentRequest) {
    state.agent_run.queue_request(pending);
}

fn attach_continuity_prompt_hint(request: &mut AgentRequest, state: &InlineState) {
    let Some(input) = request.user_input.as_deref() else {
        return;
    };
    let Some(hint) = continuity_prompt_hint(state, input) else {
        return;
    };
    if !request
        .context_hints
        .iter()
        .any(|existing| existing == &hint)
    {
        request.context_hints.push(hint);
    }
}

pub(crate) fn stop_active_agent_run_without_rendering<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    state.agent_run.held_events.clear();
    let Some(mut active_run) = state.agent_run.active.take() else {
        return Ok(());
    };

    active_run.handle.cancel();
    active_run.status_animation.clear(output)?;
    active_run.held_events.clear();
    active_run.deferred_events.clear();
    active_run.cosh_request_filter.clear();
    active_run.pending_cosh_requests.clear();
    active_run.pending_cosh_request_audits.clear();
    output.flush()?;
    Ok(())
}

pub(super) fn has_queued_run_before_held_text(state: &InlineState) -> bool {
    state
        .agent_run
        .queued_requests
        .iter()
        .any(|pending| pending.before_held_text)
}
