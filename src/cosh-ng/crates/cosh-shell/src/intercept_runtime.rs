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

            if let Some(ctrl_req_id) = &answer_run.control_request_id {
                if let Some(active_run) = state.active_run.as_ref() {
                    let response = cosh_shell::QuestionResponse {
                        request_id: ctrl_req_id.clone(),
                        answer: answer_run.answer.clone(),
                    };
                    let _ = active_run.handle.respond_question(response);
                    output.flush()?;
                    continue;
                }
            }

            stop_active_agent_run_without_rendering(state, output)?;
            state.needs_prompt_after_agent_run = event.cwd.is_none();
            start_agent_run(&answer_run.request, adapter, state, output, Some(idx))?;
            output.flush()?;
            continue;
        }

        if let Some(mut request) = agent_request_from_intercepted_input(event, idx, true) {
            let before_ms = event.started_at_ms.unwrap_or(u64::MAX);
            let ctx_config = cosh_shell::ContextWindowConfig::default();
            let ctx_entries = cosh_shell::build_context_window(blocks, before_ms, &ctx_config);
            request.context_blocks = cosh_shell::context_blocks_from_entries(&ctx_entries);
            request.context_hints = command_hook_hints_for_blocks(state, &request.context_blocks);
            state.needs_prompt_after_agent_run = event.cwd.is_none();
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
