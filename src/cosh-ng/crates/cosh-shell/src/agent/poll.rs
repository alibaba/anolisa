use std::time::Duration;

use crate::agent::continuation::{
    render_fresh_turn_recovery_notice, run_request_is_analysis_only_continuation,
    shell_handoff_first_text_fallback_request,
};
use crate::agent::events::{
    active_run_has_unrendered_interaction, render_active_agent_event,
    render_new_agent_structured_events, state_has_pending_interaction, TextHoldReason,
};
use crate::agent::finish::finish_active_agent_run;
use crate::agent::heartbeat::render_agent_heartbeat;
use crate::agent::run::{has_queued_run_before_held_text, start_agent_run, ActiveAgentRun};
use crate::approval::broker::{provider_deny_response, ProviderResponseInput};
use crate::runtime::evidence_delivery::stalled_provider_shell_handoff_continuation_request;
use crate::runtime::prelude::*;

const DEFAULT_SHELL_EVIDENCE_IDLE_TIMEOUT_SECS: u64 = 15;

pub(crate) fn poll_active_agent_run<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    poll_active_agent_run_with_policy(state, output, adapter, false, true, true, false)
}

pub(crate) fn poll_active_agent_run_deferred<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<()> {
    if let Some(active_run) = state.agent_run.active.as_mut() {
        active_run.status_animation.clear(output)?;
        output.flush()?;
    }
    poll_active_agent_run_with_policy(state, output, adapter, true, false, false, true)
}

fn poll_active_agent_run_with_policy<W: Write>(
    state: &mut InlineState,
    output: &mut W,
    adapter: &AdapterInstance,
    force_hold_output: bool,
    render_structured: bool,
    finish_completed: bool,
    suppress_heartbeat: bool,
) -> std::io::Result<()> {
    let mut should_finish = false;
    let mut first_text_fallback: Option<(AgentRequest, Option<usize>)> = None;
    loop {
        let pending_interaction_before_poll = state_has_pending_interaction(state);
        let queued_before_held_text = has_queued_run_before_held_text(state);
        let unrendered_interaction_pending = state
            .agent_run
            .active
            .as_ref()
            .is_some_and(active_run_has_unrendered_interaction);
        let shell_handoff_in_progress = state.control.shell_handoff().has_active_handoff();
        let deny_shell_during_recovery =
            state.agent_run.active.as_ref().is_some_and(|active_run| {
                run_request_is_analysis_only_continuation(Some(&active_run.request))
            });
        let analysis_only_recovery_pending = deny_shell_during_recovery;
        let provider_native_shell_tool_call_pending = adapter.capabilities().control_protocol
            && state
                .agent_run
                .active
                .as_ref()
                .is_some_and(active_run_has_unrendered_provider_native_shell_tool_call);
        let provider_native_shell_transcript_pending = adapter.capabilities().control_protocol
            && state
                .agent_run
                .active
                .as_ref()
                .is_some_and(active_run_has_unrendered_provider_native_shell_transcript);
        let provider_native_shell_result_pending = adapter.capabilities().control_protocol
            && state.agent_run.active.as_ref().is_some_and(|active_run| {
                active_run_has_pending_provider_native_shell_result(active_run, state)
            });
        let provider_native_shell_result_idle = provider_native_shell_result_pending
            && state
                .agent_run
                .active
                .as_ref()
                .is_some_and(active_run_has_stalled_shell_evidence_delivery);
        let provider_shell_activity_pending = shell_handoff_in_progress
            || provider_native_shell_tool_call_pending
            || provider_native_shell_transcript_pending
            || (provider_native_shell_result_pending && !provider_native_shell_result_idle);
        let stalled_provider_shell_fallback =
            should_start_stalled_provider_shell_fallback(StalledProviderShellFallbackInputs {
                provider_shell_activity_pending,
                pending_interaction: pending_interaction_before_poll,
                queued_before_held_text,
                unrendered_interaction: unrendered_interaction_pending,
                active_run_idle: state
                    .agent_run
                    .active
                    .as_ref()
                    .is_some_and(active_run_has_stalled_shell_evidence_delivery),
            })
            .then(|| stalled_provider_shell_handoff_continuation_request(state))
            .flatten();
        let poll_timeout = if provider_native_shell_tool_call_pending
            || provider_native_shell_transcript_pending
            || provider_native_shell_result_pending
            || analysis_only_recovery_pending
        {
            Duration::from_millis(100)
        } else if state.agent_run.host_executed_shell_result_delivered
            && !pending_interaction_before_poll
            && !queued_before_held_text
        {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(0)
        };
        let Some(active_run) = state.agent_run.active.as_mut() else {
            return Ok(());
        };
        if active_run.completed {
            active_run.status_animation.clear(output)?;
            should_finish = finish_completed;
            break;
        }

        let event = match active_run.handle.poll_event_timeout(poll_timeout) {
            Ok(AgentRunPoll::Event(event)) => event,
            Ok(AgentRunPoll::Timeout) => {
                if let Some(fallback) = stalled_provider_shell_fallback {
                    first_text_fallback = Some((fallback, active_run.selectable_after_event_index));
                    break;
                }
                if !pending_interaction_before_poll
                    && !queued_before_held_text
                    && !unrendered_interaction_pending
                {
                    if let Some(fallback) = shell_handoff_first_text_fallback_request(active_run) {
                        first_text_fallback =
                            Some((fallback, active_run.selectable_after_event_index));
                        break;
                    }
                }
                if pending_interaction_before_poll
                    || queued_before_held_text
                    || unrendered_interaction_pending
                {
                    active_run.status_animation.clear(output)?;
                    output.flush()?;
                    break;
                }
                render_agent_heartbeat(
                    active_run,
                    output,
                    suppress_heartbeat || shell_handoff_in_progress,
                )?;
                output.flush()?;
                break;
            }
            Ok(AgentRunPoll::Finished) => {
                should_finish = true;
                break;
            }
            Err(err) => AgentEvent::AgentFailed {
                run_id: active_run.request.id.clone(),
                error: err.message,
            },
        };

        let terminal_event = matches!(
            event,
            AgentEvent::AgentCompleted { .. }
                | AgentEvent::AgentFailed { .. }
                | AgentEvent::AgentCancelled { .. }
        );
        let deny_reentrant_shell_request = deny_shell_during_recovery;
        deny_reentrant_shell_request_after_foreground_evidence(
            active_run,
            &event,
            deny_reentrant_shell_request,
        );
        let provider_progress_observed = shell_evidence_provider_progress_observed(&event);
        let text_hold_reason = text_hold_reason_for_poll(TextHoldInputs {
            pending_interaction_before_poll,
            queued_before_held_text,
            unrendered_interaction: unrendered_interaction_pending,
            provider_native_shell_transcript_pending,
            provider_native_shell_result_pending,
            force_hold_output,
        });
        render_active_agent_event(active_run, event, output, text_hold_reason)?;
        if provider_progress_observed {
            state
                .evidence
                .mark_provider_progress_observed(terminal_event);
        }
        output.flush()?;
        if terminal_event {
            active_run.status_animation.clear(output)?;
            active_run.completed = true;
            should_finish = finish_completed;
            break;
        }
    }

    if let Some((fallback, selectable_after_event_index)) = first_text_fallback {
        if let Some(mut active_run) = state.agent_run.active.take() {
            active_run.handle.cancel();
            active_run.status_animation.clear(output)?;
        }
        render_fresh_turn_recovery_notice(state, output)?;
        start_agent_run(
            &fallback,
            adapter,
            state,
            output,
            selectable_after_event_index,
        )?;
        return Ok(());
    }

    if render_structured {
        render_new_agent_structured_events(state, output, adapter)?;
        output.flush()?;
    }

    if should_finish {
        finish_active_agent_run(state, output, adapter)?;
    }

    Ok(())
}

fn active_run_has_stalled_shell_evidence_delivery(active_run: &ActiveAgentRun) -> bool {
    active_run.last_activity_at.elapsed() >= shell_evidence_idle_timeout()
}

fn shell_evidence_idle_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("COSH_SHELL_EVIDENCE_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SHELL_EVIDENCE_IDLE_TIMEOUT_SECS),
    )
}

#[derive(Debug, Clone, Copy, Default)]
struct StalledProviderShellFallbackInputs {
    provider_shell_activity_pending: bool,
    pending_interaction: bool,
    queued_before_held_text: bool,
    unrendered_interaction: bool,
    active_run_idle: bool,
}

fn should_start_stalled_provider_shell_fallback(
    inputs: StalledProviderShellFallbackInputs,
) -> bool {
    inputs.active_run_idle
        && !inputs.provider_shell_activity_pending
        && !inputs.pending_interaction
        && !inputs.queued_before_held_text
        && !inputs.unrendered_interaction
}

fn active_run_has_unrendered_provider_native_shell_tool_call(active_run: &ActiveAgentRun) -> bool {
    active_run.governed_events[active_run.rendered_governed_event_count..]
        .iter()
        .any(|event| {
            matches!(
                &event.event,
                AgentEvent::ToolCall { name, .. } if is_shell_tool_name(name)
            )
        })
}

fn active_run_has_unrendered_provider_native_shell_transcript(active_run: &ActiveAgentRun) -> bool {
    let shell_tool_ids = active_run
        .governed_events
        .iter()
        .filter_map(|event| match &event.event {
            AgentEvent::ToolCall {
                tool_id: Some(tool_id),
                name,
                ..
            } if is_shell_tool_name(name) => Some(tool_id.as_str()),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();

    active_run.governed_events[active_run.rendered_governed_event_count..]
        .iter()
        .any(|event| match &event.event {
            AgentEvent::ToolOutputDelta { tool_id, .. }
            | AgentEvent::ToolCompleted { tool_id, .. } => {
                shell_tool_ids.contains(tool_id.as_str())
            }
            _ => false,
        })
}

fn deny_reentrant_shell_request_after_foreground_evidence(
    active_run: &ActiveAgentRun,
    event: &AgentEvent,
    deny_shell_after_foreground_evidence: bool,
) {
    if !deny_shell_after_foreground_evidence {
        return;
    }
    let AgentEvent::ToolPermissionRequest {
        request_id,
        tool_name,
        tool_input,
        tool_use_id,
        ..
    } = event
    else {
        return;
    };
    if !is_shell_tool_name(tool_name) {
        return;
    }
    let _ = active_run.handle.respond_approval(provider_deny_response(
        ProviderResponseInput {
            request_id,
            tool_use_id: Some(tool_use_id),
            tool_input: Some(tool_input),
        },
        "The foreground shell command already completed and its output was injected. Summarize the existing shell evidence or ask the user to start a new request before running another shell command.".to_string(),
    ));
}

fn active_run_has_pending_provider_native_shell_result(
    active_run: &ActiveAgentRun,
    state: &InlineState,
) -> bool {
    active_run.governed_events.iter().any(|event| {
        let AgentEvent::ToolCall {
            tool_id: Some(tool_id),
            name,
            ..
        } = &event.event
        else {
            return false;
        };
        is_shell_tool_name(name)
            && !active_run
                .governed_events
                .iter()
                .any(|event| matches!(&event.event, AgentEvent::ToolCompleted { tool_id: completed_tool_id, .. } if completed_tool_id == tool_id))
            && !state.control.provider_shell_transcript_output_seen(tool_id)
    })
}

fn shell_evidence_provider_progress_observed(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TextDelta { .. }
            | AgentEvent::ToolCall { .. }
            | AgentEvent::ToolPermissionRequest { .. }
            | AgentEvent::ToolOutputDelta { .. }
            | AgentEvent::ToolCompleted { .. }
            | AgentEvent::UserQuestion { .. }
            | AgentEvent::AgentCompleted { .. }
            | AgentEvent::AgentFailed { .. }
            | AgentEvent::AgentCancelled { .. }
    )
}

#[derive(Debug, Clone, Copy, Default)]
struct TextHoldInputs {
    pending_interaction_before_poll: bool,
    queued_before_held_text: bool,
    unrendered_interaction: bool,
    provider_native_shell_transcript_pending: bool,
    provider_native_shell_result_pending: bool,
    force_hold_output: bool,
}

fn text_hold_reason_for_poll(inputs: TextHoldInputs) -> Option<TextHoldReason> {
    if inputs.pending_interaction_before_poll {
        return Some(TextHoldReason::InteractionPending);
    }
    if inputs.queued_before_held_text {
        return Some(TextHoldReason::QueuedBeforeHeldText);
    }
    if inputs.unrendered_interaction {
        return Some(TextHoldReason::UnrenderedInteraction);
    }
    if inputs.provider_native_shell_transcript_pending {
        return Some(TextHoldReason::PostToolShellTranscript);
    }
    if inputs.provider_native_shell_result_pending {
        return Some(TextHoldReason::PostToolShellResult);
    }
    if inputs.force_hold_output {
        return Some(TextHoldReason::ForcedDeferredPoll);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn test_active_run() -> ActiveAgentRun {
        let request = AgentRequest {
            id: "request-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: CommandBlock {
                id: "cmd-1".to_string(),
                session_id: "session-1".to_string(),
                command: "df -h".to_string(),
                origin: Default::default(),
                cwd: "/tmp".to_string(),
                end_cwd: "/tmp".to_string(),
                started_at_ms: 1,
                ended_at_ms: 2,
                duration_ms: 1,
                exit_code: 0,
                status: CommandStatus::Completed,
                output: OutputRefs {
                    terminal_output_ref: None,
                    terminal_output_bytes: 0,
                },
            },
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("df -h".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        };
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let handle = adapter.start_cancellable(request.clone(), CoshApprovalMode::Recommend);
        let renderer = RatatuiInlineRenderer::for_terminal();
        ActiveAgentRun {
            request,
            handle,
            provider_name: "fake",
            language: Language::EnUs,
            renderer: renderer.clone(),
            status_animation: renderer.status_animation(),
            markdown_stream: renderer.stream_markdown_agent(),
            governed_events: Vec::new(),
            deferred_events: Vec::new(),
            held_events: Vec::new(),
            cosh_request_filter: crate::evidence::stream::CoshRequestStreamFilter::default(),
            pending_cosh_requests: Vec::new(),
            pending_cosh_request_audits: Vec::new(),
            rendered_governed_event_count: 0,
            selectable_after_event_index: None,
            started_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            current_phase: String::new(),
            current_message: String::new(),
            has_visible_text_delta: false,
            completed: false,
            pending_hook_notifications: Vec::new(),
        }
    }

    #[test]
    fn stalled_shell_evidence_delivery_uses_last_activity_idle_time() {
        let mut active_run = test_active_run();
        active_run.started_at = Instant::now() - Duration::from_secs(60);
        active_run.last_activity_at = Instant::now();
        active_run.has_visible_text_delta = true;

        assert!(!active_run_has_stalled_shell_evidence_delivery(&active_run));

        active_run.last_activity_at = Instant::now() - Duration::from_secs(16);

        assert!(active_run_has_stalled_shell_evidence_delivery(&active_run));
    }

    #[test]
    fn stalled_shell_fallback_waits_for_pending_interaction_to_close() {
        assert!(!should_start_stalled_provider_shell_fallback(
            StalledProviderShellFallbackInputs {
                active_run_idle: true,
                pending_interaction: true,
                ..StalledProviderShellFallbackInputs::default()
            }
        ));
        assert!(!should_start_stalled_provider_shell_fallback(
            StalledProviderShellFallbackInputs {
                active_run_idle: true,
                unrendered_interaction: true,
                ..StalledProviderShellFallbackInputs::default()
            }
        ));
        assert!(!should_start_stalled_provider_shell_fallback(
            StalledProviderShellFallbackInputs {
                active_run_idle: true,
                queued_before_held_text: true,
                ..StalledProviderShellFallbackInputs::default()
            }
        ));
    }

    #[test]
    fn stalled_shell_fallback_starts_only_when_idle_and_clear() {
        assert!(!should_start_stalled_provider_shell_fallback(
            StalledProviderShellFallbackInputs::default()
        ));
        assert!(!should_start_stalled_provider_shell_fallback(
            StalledProviderShellFallbackInputs {
                active_run_idle: true,
                provider_shell_activity_pending: true,
                ..StalledProviderShellFallbackInputs::default()
            }
        ));
        assert!(should_start_stalled_provider_shell_fallback(
            StalledProviderShellFallbackInputs {
                active_run_idle: true,
                ..StalledProviderShellFallbackInputs::default()
            }
        ));
    }

    #[test]
    fn shell_evidence_progress_includes_tool_events() {
        assert!(shell_evidence_provider_progress_observed(
            &AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: Some("tool-1".to_string()),
                name: "run_shell_command".to_string(),
                input: "df -h".to_string(),
            }
        ));
        assert!(shell_evidence_provider_progress_observed(
            &AgentEvent::ToolOutputDelta {
                run_id: "run-1".to_string(),
                tool_id: "tool-1".to_string(),
                stream: "stdout".to_string(),
                text: "output".to_string(),
            }
        ));
        assert!(shell_evidence_provider_progress_observed(
            &AgentEvent::ToolCompleted {
                run_id: "run-1".to_string(),
                tool_id: "tool-1".to_string(),
                status: "success".to_string(),
            }
        ));
    }

    #[test]
    fn text_hold_reason_none_for_plain_text_streaming() {
        assert_eq!(text_hold_reason_for_poll(TextHoldInputs::default()), None);
    }

    #[test]
    fn text_hold_reason_separates_interaction_and_post_tool_holds() {
        assert_eq!(
            text_hold_reason_for_poll(TextHoldInputs {
                pending_interaction_before_poll: true,
                provider_native_shell_result_pending: true,
                ..TextHoldInputs::default()
            }),
            Some(TextHoldReason::InteractionPending)
        );
        assert_eq!(
            text_hold_reason_for_poll(TextHoldInputs {
                provider_native_shell_result_pending: true,
                ..TextHoldInputs::default()
            }),
            Some(TextHoldReason::PostToolShellResult)
        );
        assert_eq!(
            text_hold_reason_for_poll(TextHoldInputs {
                provider_native_shell_transcript_pending: true,
                ..TextHoldInputs::default()
            }),
            Some(TextHoldReason::PostToolShellTranscript)
        );
    }
}
