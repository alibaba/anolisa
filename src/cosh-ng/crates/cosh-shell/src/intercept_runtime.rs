use super::*;

pub(super) fn render_intercept_agent_guidance<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        if !is_standalone_agent_intercept(event) {
            continue;
        }

        let key = stable_event_key("intercept", idx, event);
        if !state.handled_intercepts.insert(key) {
            continue;
        }

        if let Some(answer_run) = agent_request_from_pending_question_answer(event, idx, state) {
            render_question_answer_notice(state, &answer_run, output)?;
            stop_active_agent_run_without_rendering(state, output)?;
            state.needs_prompt_after_agent_run = true;
            start_agent_run(&answer_run.request, adapter, state, output, Some(idx))?;
            output.flush()?;
            continue;
        }

        if let Some(mut request) = agent_request_from_intercepted_input(event, idx, true) {
            request.context_blocks = recent_context_blocks(blocks, event);
            request.context_hints = command_hook_hints_for_blocks(state, &request.context_blocks);
            state.needs_prompt_after_agent_run = true;
            start_agent_run(&request, adapter, state, output, Some(idx))?;
        }
        output.flush()?;
    }

    Ok(())
}

fn is_standalone_agent_intercept(event: &ShellEvent) -> bool {
    event.kind == ShellEventKind::UserInputIntercepted
        && matches!(
            event.component.as_deref(),
            Some("natural_language") | Some("agent_marker")
        )
}

fn recent_context_blocks(blocks: &[CommandBlock], event: &ShellEvent) -> Vec<CommandBlock> {
    let before_event = event.started_at_ms.unwrap_or(u64::MAX);
    let mut recent = blocks
        .iter()
        .filter(|block| block.ended_at_ms <= before_event)
        .rev()
        .take(3)
        .cloned()
        .collect::<Vec<_>>();
    recent.reverse();
    recent
}
