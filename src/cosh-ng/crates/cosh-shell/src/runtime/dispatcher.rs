use std::io::Write;

use crate::activity::runtime::{record_approved_shell_handoff_blocks, render_activity_rows};
use crate::agent::events::flush_held_agent_events;
use crate::agent::failed_command::{
    block_end_event_index, render_failed_command_cards, render_post_failure_actions,
    should_analyze_failed_block, start_agent_for_block, FailedCommandAgentStartOptions,
    FailedCommandAnalysisTrigger,
};
use crate::agent::intercept::render_intercept_agent_guidance;
use crate::agent::poll::{poll_active_agent_run, poll_active_agent_run_deferred};
use crate::agent::run::{start_agent_run, stop_active_agent_run_without_rendering};
use crate::approval::runtime::render_approval_actions;
use crate::hooks::runtime::{
    handle_consultation_events, record_blocks_followed_by_user_input, record_command_hook_findings,
    render_queued_hook_consultation, render_recorded_hook_findings,
};
use crate::question::runtime::{
    render_question_answer_actions, render_question_cancel_actions, render_question_focus_actions,
    render_question_input_actions, render_question_toggle_actions,
};
use crate::recommendation::runtime::render_selection_actions;
use crate::runtime::cancel::render_agent_cancel_actions;
use crate::runtime::details::render_runtime_details_card_actions;
use crate::runtime::evidence_delivery::shell_handoff_continuation_requests;
use crate::runtime::evidence_requests::render_evidence_request_actions;
use crate::runtime::state::InlineState;
use crate::slash::runtime::render_slash_actions;

use super::controller::{pending_card_capture, shell_has_active_foreground_command};
use super::events::{ShellEventBatch, ShellEventCursor, ShellEventSnapshot};
use super::startup::render_startup_banner;

pub(crate) enum RuntimeAction {
    AdvanceEventCursor(ShellEventCursor),
}

pub(crate) fn stable_event_key(
    prefix: &str,
    idx: usize,
    event: &cosh_shell::types::ShellEvent,
) -> String {
    match event.started_at_ms {
        Some(started_at_ms) => format!(
            "{prefix}:{}:{}:{}",
            started_at_ms,
            event.component.as_deref().unwrap_or_default(),
            event.input.as_deref().unwrap_or_default()
        ),
        None => format!("{prefix}:{idx}"),
    }
}

pub(crate) struct RuntimeDispatcher;
pub(crate) struct QuestionConsumer;
pub(crate) struct SlashConsumer;
pub(crate) struct ApprovalConsumer;
pub(crate) struct ActivityConsumer;
pub(crate) struct EvidenceRequestConsumer;

impl RuntimeDispatcher {
    pub(crate) fn dispatch_inline_batch<W: Write>(
        snapshot: &ShellEventSnapshot,
        adapter: &cosh_shell::AdapterInstance,
        shell_label: &str,
        state: &mut InlineState,
        output: &mut W,
    ) -> std::io::Result<Vec<RuntimeAction>> {
        let batch = snapshot.batch_since(state.control.event_cursor());
        render_inline_guidance_from_batch(snapshot, &batch, adapter, shell_label, state, output)?;
        Ok(vec![RuntimeAction::AdvanceEventCursor(batch.to)])
    }

    pub(crate) fn apply_actions(actions: Vec<RuntimeAction>, state: &mut InlineState) {
        for action in actions {
            match action {
                RuntimeAction::AdvanceEventCursor(cursor) => {
                    state.control.set_event_cursor(cursor);
                }
            }
        }
    }
}

fn render_inline_guidance_from_batch<W: Write>(
    snapshot: &ShellEventSnapshot,
    batch: &ShellEventBatch,
    adapter: &cosh_shell::AdapterInstance,
    shell_label: &str,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let events = snapshot.events();
    let action_events = batch.events.as_slice();
    let event_index_base = batch.global_index(0);
    state.shell_exited = events
        .iter()
        .any(|event| event.kind == cosh_shell::types::ShellEventKind::ShellExited);
    let ledger = cosh_shell::ledger::build_command_blocks(events);
    state.session_blocks = ledger.blocks.clone();
    if state.shell_exited {
        stop_active_agent_run_without_rendering(state, output)?;
        return Ok(());
    }
    let shell_busy = shell_has_active_foreground_command(events);
    if shell_busy {
        let slash_actions = SlashConsumer::consume(
            action_events,
            &ledger.blocks,
            adapter,
            state,
            output,
            event_index_base,
        )?;
        RuntimeDispatcher::apply_actions(slash_actions, state);
        render_runtime_details_card_actions(
            action_events,
            &ledger.blocks,
            state,
            output,
            event_index_base,
        )?;
        poll_active_agent_run_deferred(state, output, adapter)?;
        return Ok(());
    }

    render_startup_banner(events, adapter, shell_label, state, output)?;
    let question_actions =
        QuestionConsumer::consume(action_events, adapter, state, output, event_index_base)?;
    RuntimeDispatcher::apply_actions(question_actions, state);
    let evidence_actions = EvidenceRequestConsumer::consume(
        action_events,
        &ledger.blocks,
        adapter,
        state,
        output,
        event_index_base,
    )?;
    RuntimeDispatcher::apply_actions(evidence_actions, state);
    let slash_actions = SlashConsumer::consume(
        action_events,
        &ledger.blocks,
        adapter,
        state,
        output,
        event_index_base,
    )?;
    RuntimeDispatcher::apply_actions(slash_actions, state);
    render_runtime_details_card_actions(
        action_events,
        &ledger.blocks,
        state,
        output,
        event_index_base,
    )?;
    let card_capture_pending = pending_card_capture(state).is_some();
    let activity_actions =
        ActivityConsumer::consume(&ledger.blocks, adapter, state, output, card_capture_pending)?;
    RuntimeDispatcher::apply_actions(activity_actions, state);
    let findings = cosh_shell::parser::findings_from_blocks(&ledger.blocks);
    record_blocks_followed_by_user_input(events, &ledger.blocks, state);
    handle_consultation_events(action_events, &ledger.blocks, adapter, state, output)?;
    render_queued_hook_consultation(state, output)?;
    record_command_hook_findings(&ledger.blocks, state);
    render_recorded_hook_findings(&ledger.blocks, state, output)?;
    render_intercept_agent_guidance(
        action_events,
        &ledger.blocks,
        adapter,
        state,
        output,
        event_index_base,
    )?;
    render_agent_cancel_actions(
        action_events,
        &ledger.blocks,
        state,
        output,
        event_index_base,
    )?;

    let analysis_mode = state.analysis_mode;
    for block in ledger
        .blocks
        .iter()
        .filter(|block| should_analyze_failed_block(block, analysis_mode))
    {
        start_agent_for_block(
            block,
            &ledger.blocks,
            &findings,
            adapter,
            state,
            output,
            FailedCommandAgentStartOptions {
                selectable_after_event_index: block_end_event_index(events, block),
                trigger: FailedCommandAnalysisTrigger::Auto,
            },
        )?;
        output.flush()?;
    }

    render_failed_command_cards(&ledger.blocks, state, output)?;

    render_post_failure_actions(
        action_events,
        &ledger.blocks,
        &findings,
        adapter,
        state,
        output,
        event_index_base,
    )?;

    render_selection_actions(action_events, state, output, event_index_base)?;
    let approval_actions = ApprovalConsumer::consume(
        action_events,
        &ledger.blocks,
        adapter,
        state,
        output,
        event_index_base,
    )?;
    RuntimeDispatcher::apply_actions(approval_actions, state);
    flush_held_agent_events(state, output)?;
    if !shell_busy && !state.control.shell_handoff().has_active_handoff() {
        poll_active_agent_run(state, output, adapter)?;
    }
    flush_held_agent_events(state, output)?;
    render_owned_shell_prompt(state, output)?;

    Ok(())
}

fn render_owned_shell_prompt<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if state.agent_run.active.is_some()
        || state.shell_exited
        || pending_card_capture(state).is_some()
    {
        return Ok(());
    }

    if !state.agent_run.needs_prompt_after_run {
        state.agent_run.native_prompt_after_run = false;
        return Ok(());
    }

    if state.agent_run.native_prompt_after_run {
        state.agent_run.needs_prompt_after_run = false;
        state.agent_run.native_prompt_after_run = false;
        return Ok(());
    }

    if std::env::var("COSH_SHELL_ISOLATED").is_ok() {
        write!(output, "cosh-osc$ ")?;
    } else {
        state.trigger_pty_prompt = true;
    }
    output.flush()?;
    state.agent_run.needs_prompt_after_run = false;
    Ok(())
}

impl QuestionConsumer {
    pub(crate) fn consume<W: Write>(
        events: &[cosh_shell::types::ShellEvent],
        adapter: &cosh_shell::AdapterInstance,
        state: &mut InlineState,
        output: &mut W,
        event_index_base: usize,
    ) -> std::io::Result<Vec<RuntimeAction>> {
        render_question_focus_actions(events, state, output, event_index_base)?;
        render_question_toggle_actions(events, state, output, event_index_base)?;
        render_question_input_actions(events, state, output, event_index_base)?;
        render_question_cancel_actions(events, state, output, event_index_base)?;
        render_question_answer_actions(events, adapter, state, output, event_index_base)?;
        Ok(Vec::new())
    }
}

impl SlashConsumer {
    pub(crate) fn consume<W: Write>(
        events: &[cosh_shell::types::ShellEvent],
        blocks: &[cosh_shell::types::CommandBlock],
        adapter: &cosh_shell::AdapterInstance,
        state: &mut InlineState,
        output: &mut W,
        event_index_base: usize,
    ) -> std::io::Result<Vec<RuntimeAction>> {
        render_slash_actions(events, blocks, adapter, state, output, event_index_base)?;
        Ok(Vec::new())
    }
}

impl ApprovalConsumer {
    pub(crate) fn consume<W: Write>(
        events: &[cosh_shell::types::ShellEvent],
        blocks: &[cosh_shell::types::CommandBlock],
        adapter: &cosh_shell::AdapterInstance,
        state: &mut InlineState,
        output: &mut W,
        event_index_base: usize,
    ) -> std::io::Result<Vec<RuntimeAction>> {
        render_approval_actions(events, blocks, adapter, state, output, event_index_base)?;
        Ok(Vec::new())
    }
}

impl EvidenceRequestConsumer {
    pub(crate) fn consume<W: Write>(
        events: &[cosh_shell::types::ShellEvent],
        blocks: &[cosh_shell::types::CommandBlock],
        adapter: &cosh_shell::AdapterInstance,
        state: &mut InlineState,
        output: &mut W,
        event_index_base: usize,
    ) -> std::io::Result<Vec<RuntimeAction>> {
        render_evidence_request_actions(events, blocks, adapter, state, output, event_index_base)?;
        Ok(Vec::new())
    }
}

impl ActivityConsumer {
    pub(crate) fn consume<W: Write>(
        blocks: &[cosh_shell::types::CommandBlock],
        adapter: &cosh_shell::AdapterInstance,
        state: &mut InlineState,
        output: &mut W,
        card_capture_pending: bool,
    ) -> std::io::Result<Vec<RuntimeAction>> {
        let handoff_activity_ids = record_approved_shell_handoff_blocks(state, blocks);
        render_activity_rows(state, &handoff_activity_ids, output)?;
        if !card_capture_pending && state.agent_run.active.is_none() {
            for request in shell_handoff_continuation_requests(state) {
                start_agent_run(&request, adapter, state, output, None)?;
            }
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosh_shell::types::ShellEvent;

    #[test]
    fn dispatcher_advances_cursor_to_snapshot_end() {
        let adapter = cosh_shell::AdapterInstance::Fake(cosh_shell::adapter::FakeAgentAdapter);
        let mut state = InlineState::default();
        let mut output = Vec::new();
        let snapshot = ShellEventSnapshot::new(&[
            ShellEvent::user_input_intercepted("s", "/help"),
            ShellEvent::user_input_intercepted("s", "/help"),
        ]);

        let actions = RuntimeDispatcher::dispatch_inline_batch(
            &snapshot,
            &adapter,
            "bash",
            &mut state,
            &mut output,
        )
        .expect("dispatch should render");
        RuntimeDispatcher::apply_actions(actions, &mut state);

        assert_eq!(
            state.control.event_cursor().position(),
            snapshot.cursor().position()
        );
    }

    #[test]
    fn stable_event_key_uses_marker_timestamp_when_available() {
        let mut event = ShellEvent::user_input_intercepted("s", "/help");
        assert_eq!(stable_event_key("slash", 7, &event), "slash:7");

        event.started_at_ms = Some(123);
        assert_eq!(stable_event_key("slash", 7, &event), "slash:123::/help");
    }
}
