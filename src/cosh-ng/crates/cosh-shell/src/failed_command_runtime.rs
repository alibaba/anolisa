use super::*;

pub(super) fn render_post_failure_actions<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    findings: &[Finding],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let key = format!("cancel-{idx}");
        if event_cancels_failed_command_analysis(event)
            && !state.handled_cancellations.contains(&key)
        {
            let Some(block) = latest_pending_failed_block_before_event(blocks, state, event) else {
                continue;
            };

            state.handled_cancellations.insert(key);
            state.canceled_blocks.insert(block.id.clone());
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Agent cancelled",
                vec![format!(
                    "cancelled pending analysis for `{}`",
                    block.command
                )],
                Some("Shell remains active."),
            )?;
            output.flush()?;
            continue;
        }

        let key = format!("confirm-{idx}");
        if !event_confirms_failed_command_analysis(event)
            || state.handled_confirmations.contains(&key)
        {
            continue;
        }

        let Some(block) = latest_pending_failed_block_before_event(blocks, state, event) else {
            continue;
        };

        state.handled_confirmations.insert(key);
        start_agent_for_block(block, blocks, findings, adapter, state, output, Some(idx))?;
        output.flush()?;
    }

    Ok(())
}

pub(super) fn latest_pending_failed_block_before_event<'a>(
    blocks: &'a [CommandBlock],
    state: &InlineState,
    event: &ShellEvent,
) -> Option<&'a CommandBlock> {
    blocks.iter().rev().find(|block| {
        should_analyze_failed_block(block, state.analysis_mode)
            && !state.analyzed_blocks.contains(&block.id)
            && !state.canceled_blocks.contains(&block.id)
            && event_happened_after_block_end(event, block)
    })
}

pub(super) fn should_analyze_failed_block(block: &CommandBlock, mode: AnalysisMode) -> bool {
    if block.exit_code == 0 || block.command.trim().is_empty() {
        return false;
    }
    if mode == AnalysisMode::Manual {
        return false;
    }
    let category = cosh_shell::classify_exit(block.exit_code, &block.command);
    match category {
        cosh_shell::ExitCodeCategory::Success
        | cosh_shell::ExitCodeCategory::UserInterrupt
        | cosh_shell::ExitCodeCategory::PipelineNormal => false,
        cosh_shell::ExitCodeCategory::CommandSpecificNormal => mode == AnalysisMode::Auto,
        _ => true,
    }
}

fn event_happened_after_block_end(event: &ShellEvent, block: &CommandBlock) -> bool {
    event
        .started_at_ms
        .map(|timestamp| timestamp >= block.ended_at_ms)
        .unwrap_or(true)
}

pub(super) fn block_end_event_index(events: &[ShellEvent], block: &CommandBlock) -> Option<usize> {
    events.iter().enumerate().find_map(|(idx, event)| {
        if event.command_id.as_deref() == Some(block.id.as_str())
            && matches!(
                event.kind,
                ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed
            )
        {
            Some(idx)
        } else {
            None
        }
    })
}

pub(super) fn start_agent_for_block<W: Write>(
    block: &CommandBlock,
    blocks: &[CommandBlock],
    findings: &[Finding],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    selectable_after_event_index: Option<usize>,
) -> std::io::Result<()> {
    if !should_analyze_failed_block(block, state.analysis_mode) {
        return Ok(());
    }

    if state.canceled_blocks.contains(&block.id) {
        return Ok(());
    }

    if !state.analyzed_blocks.insert(block.id.clone()) {
        return Ok(());
    }

    if state.active_run.is_some() {
        state.analyzed_blocks.remove(&block.id);
        if state.queued_analysis_notices.insert(block.id.clone()) {
            RatatuiInlineRenderer::for_terminal().write_notice(
                output,
                "Agent queued",
                vec![
                    format!("Captured failed command: {}", block.command),
                    "Current Agent run is still streaming.".to_string(),
                ],
                Some("This failure will be analyzed after the current Agent run finishes."),
            )?;
        }
        return Ok(());
    }

    match agent_request_after_confirmation(&block.session_id, block, findings, true) {
        Some(mut request) => {
            let ctx_config = cosh_shell::ContextWindowConfig::default();
            let ctx_entries =
                cosh_shell::build_context_window(blocks, block.ended_at_ms, &ctx_config);
            request.context_blocks = cosh_shell::context_blocks_from_entries(&ctx_entries);
            request.context_hints = command_hook_hints_for_block(state, block);
            start_agent_run(
                &request,
                adapter,
                state,
                output,
                selectable_after_event_index,
            )
        }
        None => Ok(()),
    }
}
