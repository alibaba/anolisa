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
            if let Some(raw) = event.input.as_deref() {
                write!(output, "\u{2192} AI: {}\n", raw.trim())?;
                output.flush()?;
            }
            render_question_answer_notice(state, &answer_run, output)?;
            stop_active_agent_run_without_rendering(state, output)?;
            state.needs_prompt_after_agent_run = true;
            start_agent_run(&answer_run.request, adapter, state, output, Some(idx))?;
            output.flush()?;
            continue;
        }

        if let Some(mut request) = agent_request_from_intercepted_input(event, idx, true) {
            if let Some(raw) = event.input.as_deref() {
                write!(output, "\u{2192} AI: {}\n", raw.trim())?;
                output.flush()?;
            }
            let before_ms = event.started_at_ms.unwrap_or(u64::MAX);
            let ctx_config = cosh_shell::ContextWindowConfig::default();
            let ctx_entries = cosh_shell::build_context_window(blocks, before_ms, &ctx_config);
            request.context_blocks = cosh_shell::context_blocks_from_entries(&ctx_entries);
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

