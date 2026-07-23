use std::io::Write;

use crate::activity::runtime::{record_approved_shell_handoff_blocks, render_activity_rows};
use crate::agent::events::flush_held_agent_events;
use crate::agent::failed_command::{
    block_end_event_index, collect_failed_command_insights, failed_command_candidate,
    failed_command_intervention, render_post_failure_actions, start_agent_for_block,
    FailedCommandAgentStartOptions, FailedCommandAnalysisTrigger,
};
use crate::agent::intercept::render_intercept_agent_guidance;
use crate::agent::poll::{poll_active_agent_run, poll_active_agent_run_deferred};
use crate::agent::run::{
    start_agent_run_with_origin, stop_active_agent_run_without_rendering, AgentStartIntent,
};
use crate::approval::runtime::render_approval_actions;
use crate::insight::model::InterventionDecision;
use crate::insight::policy::InterventionGates;
use crate::question::runtime::{
    render_question_answer_actions, render_question_cancel_actions, render_question_focus_actions,
    render_question_input_actions, render_question_toggle_actions,
};
use crate::recommendation::personal_integration::record_completed_command_blocks;
use crate::recommendation::runtime::render_selection_actions;
use crate::runtime::cancel::render_agent_cancel_actions;
use crate::runtime::details::render_runtime_details_card_actions;
use crate::runtime::evidence_delivery::shell_handoff_continuation_requests;
use crate::runtime::evidence_requests::render_evidence_request_actions;
use crate::runtime::hooks::{
    handle_consultation_events, record_blocks_followed_by_user_input, record_command_hook_findings,
    render_queued_hook_consultation, render_recorded_hook_findings,
};
use crate::runtime::insight::render_pending_command_insight;
use crate::runtime::prelude::{
    build_command_blocks, findings_from_blocks, AdapterInstance, CommandBlock, ShellEvent,
    ShellEventKind,
};
use crate::runtime::state::InlineState;
use crate::slash::runtime::render_slash_actions;
use crate::slash::session::poll_background_compaction;

use super::controller::{pending_card_capture, shell_has_active_foreground_command};
use super::events::{ShellEventBatch, ShellEventCursor, ShellEventSnapshot};
use super::startup::{
    render_pending_recommendation_notice, render_startup_banner, render_startup_health_banner,
};

pub(crate) enum RuntimeAction {
    AdvanceEventCursor(ShellEventCursor),
}

pub(crate) fn stable_event_key(prefix: &str, idx: usize, event: &ShellEvent) -> String {
    match event.started_at_ms {
        Some(started_at_ms) if event.component.as_deref() == Some("card_secret") => {
            format!("{prefix}:{started_at_ms}:card_secret:{idx}")
        }
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
        adapter: &AdapterInstance,
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
    adapter: &AdapterInstance,
    shell_label: &str,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    state.personalization.poll_ready();
    let events = snapshot.events();
    let action_events = batch.events.as_slice();
    let event_index_base = batch.global_index(0);
    state.shell_exited = events
        .iter()
        .any(|event| event.kind == ShellEventKind::ShellExited);
    let ledger = build_command_blocks(events);
    record_completed_command_blocks(state, &ledger.blocks);
    state.session_blocks = ledger.blocks.clone();
    if state.shell_exited {
        if let Some(event) = events
            .iter()
            .rev()
            .find(|event| event.kind == ShellEventKind::ShellExited)
        {
            crate::agent::intercept::finalize_personal_prompt_feedback_on_exit(event, state);
        }
        stop_active_agent_run_without_rendering(state, output)?;
        return Ok(());
    }
    let question_actions =
        QuestionConsumer::consume(action_events, adapter, state, output, event_index_base)?;
    RuntimeDispatcher::apply_actions(question_actions, state);
    crate::auth::runtime::render_auth_card_actions(
        action_events,
        adapter,
        state,
        output,
        event_index_base,
    )?;
    let evidence_actions = EvidenceRequestConsumer::consume(
        action_events,
        &ledger.blocks,
        adapter,
        state,
        output,
        event_index_base,
    )?;
    RuntimeDispatcher::apply_actions(evidence_actions, state);
    let approval_actions = ApprovalConsumer::consume(
        action_events,
        &ledger.blocks,
        adapter,
        state,
        output,
        event_index_base,
    )?;
    RuntimeDispatcher::apply_actions(approval_actions, state);
    let shell_busy = shell_has_active_foreground_command(events);
    if shell_busy {
        if let Some(cancellation) = state.personalization.analyzer_cancellation.as_ref() {
            cancellation.set_foreground_idle(false);
        }
        state.personalization.idle_since = None;
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
        // Foreground output is active: harvest compactor results but defer
        // completion rendering to the next safe prompt boundary.
        poll_background_compaction(state, output, adapter, true)?;
        return Ok(());
    }

    render_startup_banner(events, adapter, shell_label, state, output)?;
    render_startup_health_banner(state, output)?;
    render_pending_recommendation_notice(state, output)?;
    update_personal_shell_input_state(action_events, state);
    let personal_idle = state.agent_run.active.is_none()
        && !state.personalization.shell_input_active
        && !action_events
            .iter()
            .any(|event| event.kind == ShellEventKind::UserInputIntercepted);
    crate::recommendation::personal_session::poll_personal_session(state, adapter, personal_idle);
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
    let findings = findings_from_blocks(&ledger.blocks);
    record_blocks_followed_by_user_input(events, &ledger.blocks, state);
    handle_consultation_events(action_events, &ledger.blocks, adapter, state, output)?;
    render_queued_hook_consultation(state, output)?;
    record_command_hook_findings(events, &ledger.blocks, state, event_index_base);
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
    let auto_runtime_available =
        state.agent_run.active.is_none() && pending_card_capture(state).is_none();
    let auto_blocks = ledger
        .blocks
        .iter()
        .rev()
        .filter(|block| {
            auto_runtime_available
                && block.origin == crate::types::CommandOrigin::UserInteractive
                && !state.hooks.block_followed_by_user_input(&block.id)
                && block_end_event_index(events, block).is_some_and(|idx| idx >= event_index_base)
        })
        .collect::<Vec<_>>();
    for block in auto_blocks {
        if state.agent_run.active.is_some() {
            break;
        }
        let Some(candidate) = failed_command_candidate(events, block) else {
            continue;
        };
        let user_has_not_continued = !state.hooks.block_followed_by_user_input(&block.id);
        let gates = InterventionGates {
            same_dispatch_batch: block_end_event_index(events, block)
                .is_some_and(|idx| idx >= event_index_base),
            input_empty: user_has_not_continued,
            foreground_idle: !shell_busy,
            active_runtime_idle: state.agent_run.active.is_none()
                && pending_card_capture(state).is_none(),
            user_has_not_continued,
            user_interactive_origin: block.origin == crate::types::CommandOrigin::UserInteractive,
            budget_available: !state.insight_budget.is_suppressed(
                &candidate.suppression_key,
                candidate.severity,
                block.ended_at_ms,
            ),
        };
        if !matches!(
            failed_command_intervention(events, block, &candidate, analysis_mode, gates),
            InterventionDecision::AutoAnalyze { .. }
        ) {
            continue;
        }
        if state.insight_budget.should_suppress(
            candidate.suppression_key,
            candidate.severity,
            block.ended_at_ms,
        ) {
            continue;
        }
        // Auto starts from the same precmd batch, whose native prompt is already cached.
        state.agent_run.native_prompt_after_run = true;
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

    collect_failed_command_insights(events, &ledger.blocks, state, output, event_index_base)?;
    if pending_card_capture(state).is_none() {
        render_pending_command_insight(state, output)?;
    } else {
        state.pending_command_insight = None;
    }

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
    flush_held_agent_events(state, output)?;
    if !shell_busy && !state.control.shell_handoff().has_active_handoff() {
        poll_active_agent_run(state, output, adapter)?;
    }
    flush_held_agent_events(state, output)?;
    poll_background_compaction(state, output, adapter, false)?;
    render_owned_shell_prompt(state, output)?;

    Ok(())
}

fn update_personal_shell_input_state(events: &[ShellEvent], state: &mut InlineState) {
    for event in events {
        match event.kind {
            ShellEventKind::ShellReady
            | ShellEventKind::CommandStarted
            | ShellEventKind::CommandCompleted
            | ShellEventKind::CommandFailed => state.personalization.shell_input_active = false,
            ShellEventKind::UserInputIntercepted
                if event.component.as_deref() == Some("shell_input") =>
            {
                state.personalization.shell_input_active =
                    event.message.as_deref() != Some("input empty");
            }
            _ => {}
        }
    }
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
        let prompt = std::env::var("COSH_POC_PS1").unwrap_or_else(|_| "cosh-osc$ ".to_string());
        write!(output, "{prompt}")?;
    } else {
        state.trigger_pty_prompt = true;
    }
    output.flush()?;
    state.agent_run.needs_prompt_after_run = false;
    Ok(())
}

impl QuestionConsumer {
    pub(crate) fn consume<W: Write>(
        events: &[ShellEvent],
        adapter: &AdapterInstance,
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
        events: &[ShellEvent],
        blocks: &[CommandBlock],
        adapter: &AdapterInstance,
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
        events: &[ShellEvent],
        blocks: &[CommandBlock],
        adapter: &AdapterInstance,
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
        events: &[ShellEvent],
        blocks: &[CommandBlock],
        adapter: &AdapterInstance,
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
        blocks: &[CommandBlock],
        adapter: &AdapterInstance,
        state: &mut InlineState,
        output: &mut W,
        card_capture_pending: bool,
    ) -> std::io::Result<Vec<RuntimeAction>> {
        let handoff_activity_ids = record_approved_shell_handoff_blocks(state, blocks);
        render_activity_rows(state, &handoff_activity_ids, output)?;
        if !card_capture_pending && state.agent_run.active.is_none() {
            for (request, origin) in shell_handoff_continuation_requests(state) {
                // Shell-handoff continuations are automatic conversation
                // resumptions, not fresh user requests.
                start_agent_run_with_origin(
                    &request,
                    origin,
                    AgentStartIntent::InternalBestEffort,
                    adapter,
                    state,
                    output,
                    None,
                )?;
            }
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::prelude::FakeAgentAdapter;

    #[test]
    fn dispatcher_advances_cursor_to_snapshot_end() {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
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

    #[test]
    fn stable_event_key_does_not_retain_secret_card_input() {
        let mut event = ShellEvent::user_input_intercepted("s", "auth-1:secret-value");
        event.started_at_ms = Some(123);
        event.component = Some("card_secret".to_string());

        let key = stable_event_key("auth", 7, &event);

        assert_eq!(key, "auth:123:card_secret:7");
        assert!(!key.contains("secret-value"));
    }

    #[test]
    fn personal_idle_tracks_whether_the_shell_input_line_is_empty() {
        let mut state = InlineState::default();
        let mut editing = ShellEvent::user_input_intercepted("s", "");
        editing.component = Some("shell_input".to_string());
        editing.message = Some("input editing".to_string());
        update_personal_shell_input_state(&[editing], &mut state);
        assert!(state.personalization.shell_input_active);

        let mut empty = ShellEvent::user_input_intercepted("s", "");
        empty.component = Some("shell_input".to_string());
        empty.message = Some("input empty".to_string());
        update_personal_shell_input_state(&[empty], &mut state);
        assert!(!state.personalization.shell_input_active);
    }

    #[test]
    fn busy_shell_updates_the_analyzer_foreground_gate() {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let cancellation =
            crate::recommendation::personal_analysis_runtime::AnalyzerCancellation::new();
        let mut state = InlineState {
            personalization: crate::recommendation::personal_state::PersonalizationState {
                analyzer_cancellation: Some(cancellation.clone()),
                ..Default::default()
            },
            ..InlineState::default()
        };
        let mut output = Vec::new();
        let snapshot = ShellEventSnapshot::new(&[ShellEvent::command_started(
            "session", "command", "sleep 1", "/tmp", 1,
        )]);

        RuntimeDispatcher::dispatch_inline_batch(
            &snapshot,
            &adapter,
            "bash",
            &mut state,
            &mut output,
        )
        .expect("dispatch should render");

        assert!(!cancellation.foreground_idle());
    }
}
