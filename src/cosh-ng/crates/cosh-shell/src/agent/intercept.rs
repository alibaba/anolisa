use crate::runtime::prelude::*;

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
        clear_dismissed_prompt_ghost_context(event, state);
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
                bind_pending_input_ghost_context(&mut request, state, event);
                if let Some(hint) = continuity_prompt_hint(state, input) {
                    request.context_hints.push(hint);
                }
            }
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

fn clear_dismissed_prompt_ghost_context(event: &ShellEvent, state: &mut InlineState) {
    if event.kind == ShellEventKind::UserInputIntercepted
        && event.component.as_deref() == Some("prompt_ghost")
        && event.message.as_deref() == Some("dismissed")
    {
        state.pending_input_ghost_binding = None;
    }
}

fn bind_pending_input_ghost_context(
    request: &mut AgentRequest,
    state: &mut InlineState,
    event: &ShellEvent,
) {
    if crate::types::request_context_binding(request) != AgentContextBinding::FreeForm {
        return;
    }
    if event.component.as_deref() != Some("prompt_ghost") {
        return;
    }
    let Some(pending) = state.pending_input_ghost_binding.take() else {
        return;
    };
    crate::types::set_request_context_binding(request, pending.binding);
}

fn is_standalone_agent_intercept(event: &ShellEvent) -> bool {
    event.kind == ShellEventKind::UserInputIntercepted
        && matches!(
            event.component.as_deref(),
            Some("natural_language") | Some("agent_marker") | Some("prompt_ghost")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::state::PendingInputGhostBinding;

    fn prompt_ghost_event(message: Option<&str>, input: Option<&str>) -> ShellEvent {
        ShellEvent {
            kind: ShellEventKind::UserInputIntercepted,
            session_id: "session-1".to_string(),
            command_id: None,
            command: None,
            cwd: None,
            end_cwd: None,
            exit_code: None,
            started_at_ms: Some(1),
            ended_at_ms: None,
            duration_ms: None,
            terminal_output_ref: None,
            terminal_output_bytes: None,
            input: input.map(str::to_string),
            component: Some("prompt_ghost".to_string()),
            message: message.map(str::to_string),
            command_origin: None,
        }
    }

    #[test]
    fn dismissed_prompt_ghost_clears_pending_binding() {
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding {
                binding: AgentContextBinding::StartupHealthFollowUp,
            }),
            ..Default::default()
        };

        clear_dismissed_prompt_ghost_context(
            &prompt_ghost_event(Some("dismissed"), None),
            &mut state,
        );

        assert!(state.pending_input_ghost_binding.is_none());
    }

    #[test]
    fn accepted_prompt_ghost_does_not_clear_pending_binding_before_binding() {
        let mut state = InlineState {
            pending_input_ghost_binding: Some(PendingInputGhostBinding {
                binding: AgentContextBinding::StartupHealthFollowUp,
            }),
            ..Default::default()
        };

        clear_dismissed_prompt_ghost_context(
            &prompt_ghost_event(
                Some("input intercepted before reaching bash"),
                Some("analyze"),
            ),
            &mut state,
        );

        assert!(state.pending_input_ghost_binding.is_some());
    }
}
