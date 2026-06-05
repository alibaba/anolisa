use crate::runtime::prelude::*;

const FREE_FORM_RECENT_CONTEXT_COMMANDS: usize = 5;

pub(crate) fn render_intercept_agent_guidance<W: Write>(
    events: &[ShellEvent],
    _blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        if !is_standalone_agent_intercept(event) {
            continue;
        }

        let key = stable_event_key("intercept", event_index, event);
        if !state.handled_intercepts.insert(key) {
            continue;
        }

        if let Some(answer_run) =
            agent_request_from_pending_question_answer(event, event_index, state)
        {
            render_question_answer_notice(state, &answer_run, output)?;
            stop_active_agent_run_without_rendering(state, output)?;
            state.agent_run.needs_prompt_after_run = event.cwd.is_none();
            start_agent_run(
                &answer_run.request,
                adapter,
                state,
                output,
                Some(event_index),
            )?;
            output.flush()?;
            continue;
        }

        if let Some(mut request) = agent_request_from_intercepted_input(event, event_index, true) {
            let user_input = request.user_input.clone();
            if let Some(input) = user_input.as_deref() {
                if let Some(hint) = continuity_prompt_hint(state, input) {
                    request.context_hints.push(hint);
                }
            }
            attach_recent_shell_context(&mut request, _blocks);
            state.agent_run.needs_prompt_after_run = event.cwd.is_none();
            start_agent_run(&request, adapter, state, output, Some(event_index))?;
            if let Some(input) = user_input.as_deref() {
                record_user_intent(state, input);
            }
        }
        output.flush()?;
    }

    Ok(())
}

fn attach_recent_shell_context(request: &mut AgentRequest, blocks: &[CommandBlock]) {
    if !request.context_blocks.is_empty() {
        return;
    }
    request.context_blocks = blocks
        .iter()
        .filter(|block| block.session_id == request.session_id)
        .rev()
        .take(FREE_FORM_RECENT_CONTEXT_COMMANDS)
        .cloned()
        .collect::<Vec<_>>();
    request.context_blocks.reverse();
}

fn is_standalone_agent_intercept(event: &ShellEvent) -> bool {
    event.kind == ShellEventKind::UserInputIntercepted
        && matches!(
            event.component.as_deref(),
            Some("natural_language") | Some("agent_marker")
        )
}
