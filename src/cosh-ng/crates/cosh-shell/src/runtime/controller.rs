use std::io::Write;
use std::time::Duration;

use crate::question::runtime::pending_question_capture;
use crate::runtime::prelude::*;
use crate::runtime::state::{ApprovalRequestStatus, CoshApprovalMode, InlineState};

use super::dispatcher::RuntimeDispatcher;
use super::events::ShellEventSnapshot;
use super::terminal::CrLfWriter;

mod bootstrap;

pub(crate) use bootstrap::{
    run_adapter_demo, run_demo, run_host_demo, run_interactive, run_interactive_demo, run_raw,
};

fn render_raw_inline_events<W: Write>(
    events: &[ShellEvent],
    output: &mut W,
    adapter: &AdapterInstance,
    shell_label: &str,
    inline_state: &mut InlineState,
) -> std::io::Result<RawObserverAction> {
    let mut terminal_output = CrLfWriter::new(output);
    let snapshot = ShellEventSnapshot::new(events);
    let actions = RuntimeDispatcher::dispatch_inline_batch(
        &snapshot,
        adapter,
        shell_label,
        inline_state,
        &mut terminal_output,
    )?;
    RuntimeDispatcher::apply_actions(actions, inline_state);
    if let Some(request) = inline_state
        .control
        .shell_handoff_mut()
        .emit_next_approved()
    {
        return Ok(RawObserverAction::EmitToPty(request));
    }
    if let Some(capture) = pending_card_capture(inline_state) {
        return Ok(RawObserverAction::CaptureInput(capture));
    }
    if inline_state.trigger_pty_prompt {
        inline_state.trigger_pty_prompt = false;
        return Ok(RawObserverAction::RestorePrompt);
    }
    let shell_busy = shell_has_active_foreground_command(snapshot.events());
    if let Some(action) =
        shell_handoff_timeout_recovery_action(inline_state, shell_busy, &mut terminal_output)?
    {
        return Ok(action);
    }
    let shell_handoff_pending = inline_state
        .control
        .shell_handoff()
        .pending_front()
        .is_some();
    if shell_busy || shell_handoff_pending {
        Ok(RawObserverAction::RawPassthrough)
    } else if inline_state
        .agent_run
        .active
        .as_ref()
        .is_some_and(|run| !run.completed)
    {
        Ok(RawObserverAction::DelayShellOutput)
    } else {
        Ok(RawObserverAction::Continue)
    }
}

fn shell_handoff_timeout_recovery_action<W: Write>(
    state: &mut InlineState,
    shell_busy: bool,
    output: &mut W,
) -> std::io::Result<Option<RawObserverAction>> {
    shell_handoff_timeout_recovery_action_with_timeout(
        state,
        shell_busy,
        output,
        configured_shell_handoff_timeout(),
    )
}

fn shell_handoff_timeout_recovery_action_with_timeout<W: Write>(
    state: &mut InlineState,
    shell_busy: bool,
    output: &mut W,
    timeout: Option<Duration>,
) -> std::io::Result<Option<RawObserverAction>> {
    let shell_handoff_pending = state.control.shell_handoff().pending_front().is_some();
    if !shell_busy && !shell_handoff_pending {
        if let Some(timeout) = state.pending_shell_handoff_timeout_notice.take() {
            render_shell_handoff_timeout_notice(state, output, timeout)?;
        }
        return Ok(None);
    }

    let Some(timeout) = timeout else {
        return Ok(None);
    };
    let marked_timeout = state
        .control
        .shell_handoff_mut()
        .mark_timeout_interrupt_if_elapsed(timeout);
    if !marked_timeout {
        return Ok(None);
    }
    state.pending_shell_handoff_timeout_notice = Some(timeout);
    Ok(Some(RawObserverAction::InterruptForeground))
}

fn render_shell_handoff_timeout_notice<W: Write>(
    state: &InlineState,
    output: &mut W,
    timeout: Duration,
) -> std::io::Result<()> {
    let i18n = state.i18n();
    let timeout_secs = timeout.as_secs().to_string();
    RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_notice_panel(
            output,
            NoticePanelModel {
                title: i18n.t(MessageId::ApprovalShellHandoffTimeoutTitle),
                body: vec![
                    i18n.format(
                        MessageId::ApprovalShellHandoffTimeoutExceededBody,
                        &[("seconds", &timeout_secs)],
                    ),
                    i18n.t(MessageId::ApprovalShellHandoffTimeoutInterruptBody)
                        .to_string(),
                ],
                footer: None,
            },
        )?;
    Ok(())
}

fn configured_shell_handoff_timeout() -> Option<Duration> {
    let secs = std::env::var("COSH_SHELL_HANDOFF_TIMEOUT_SECS")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

#[cfg(test)]
pub(crate) fn render_inline_guidance<W: Write>(
    events: &[ShellEvent],
    adapter: &AdapterInstance,
    shell_label: &str,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let snapshot = ShellEventSnapshot::new(events);
    let previous_cursor = state.control.event_cursor();
    state.control.set_event_cursor(Default::default());
    let actions =
        RuntimeDispatcher::dispatch_inline_batch(&snapshot, adapter, shell_label, state, output)?;
    RuntimeDispatcher::apply_actions(actions, state);
    state.control.set_event_cursor(previous_cursor);
    Ok(())
}

fn approval_mode_from_config(value: &str) -> CoshApprovalMode {
    match value {
        "recommend" | "suggest" => CoshApprovalMode::Recommend,
        "trust" => CoshApprovalMode::Trust,
        _ => CoshApprovalMode::Auto,
    }
}

pub(crate) fn pending_card_capture(state: &InlineState) -> Option<RawInputCapture> {
    if let Some(mode_panel) = state.control.pending_mode_panel() {
        return Some(RawInputCapture::Mode {
            id: mode_panel.id.clone(),
            option_count: 3,
            selected: mode_panel.selected_option,
        });
    }
    if let Some(config_panel) = state.control.pending_config_panel() {
        return Some(RawInputCapture::Config {
            id: config_panel.id.clone(),
            option_count: 2,
            selected: config_panel.selected_option,
        });
    }
    if let Some(config_language_panel) = state.control.pending_config_language_panel() {
        return Some(RawInputCapture::ConfigLanguage {
            id: config_language_panel.id.clone(),
            option_count: 3,
            selected: config_language_panel.selected_option,
        });
    }

    if state.agent_run.active.is_none() {
        if let Some(consultation) = state.hooks.pending_consultation.as_ref() {
            return Some(RawInputCapture::Consultation {
                id: consultation.card_id.clone(),
            });
        }
    }

    if let Some(capture) = pending_question_capture(state) {
        return Some(capture);
    }

    if let Some(capture) = crate::auth::runtime::pending_auth_capture(state) {
        return Some(capture);
    }

    if let Some(capture) = crate::runtime::evidence_requests::pending_evidence_capture(state) {
        return Some(capture);
    }

    state
        .approvals
        .requests
        .iter()
        .find(|request| request.status == ApprovalRequestStatus::Pending)
        .map(|request| RawInputCapture::Approval {
            id: request.id.clone(),
            is_hook: request.subject.contains("HOOK:"),
        })
}

pub(crate) fn shell_has_active_foreground_command(events: &[ShellEvent]) -> bool {
    let mut active = std::collections::HashSet::new();
    for event in events {
        let Some(command_id) = event.command_id.as_ref() else {
            continue;
        };

        match event.kind {
            ShellEventKind::CommandStarted => {
                active.insert(command_id.as_str());
            }
            ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed => {
                active.remove(command_id.as_str());
            }
            _ => {}
        }
    }

    !active.is_empty()
}

#[cfg(test)]
mod hook_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::run::ActiveAgentRun;
    use std::time::Instant;

    #[test]
    fn approval_mode_config_keeps_legacy_suggest_as_recommend() {
        assert_eq!(
            approval_mode_from_config("recommend"),
            CoshApprovalMode::Recommend
        );
        assert_eq!(
            approval_mode_from_config("suggest"),
            CoshApprovalMode::Recommend
        );
        assert_eq!(approval_mode_from_config("trust"), CoshApprovalMode::Trust);
        assert_eq!(approval_mode_from_config("auto"), CoshApprovalMode::Auto);
        assert_eq!(approval_mode_from_config("unknown"), CoshApprovalMode::Auto);
    }

    #[test]
    fn active_foreground_command_keeps_raw_passthrough_even_when_agent_running() {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState::default();
        state.agent_run.active = Some(test_active_run());
        let events = vec![ShellEvent::command_started(
            "session-1",
            "cmd-1",
            "sudo df -h",
            "/tmp",
            10,
        )];
        let mut output = Vec::new();

        let action = render_raw_inline_events(&events, &mut output, &adapter, "zsh", &mut state)
            .expect("render raw inline events");

        assert_eq!(action, RawObserverAction::RawPassthrough);
    }

    #[test]
    fn pending_shell_handoff_keeps_raw_passthrough_before_preexec() {
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let mut state = InlineState::default();
        state.agent_run.active = Some(test_active_run());
        let request = ShellHandoffRequest::new(
            "echo approved",
            "$ echo approved",
            "approved_provider_shell_tool",
            "user",
            "req-approved",
            "run-approved",
            1,
        )
        .expect("handoff request");
        state
            .control
            .shell_handoff_mut()
            .enqueue_approved_request(request.clone());
        let mut first_output = Vec::new();

        let first_action =
            render_raw_inline_events(&[], &mut first_output, &adapter, "zsh", &mut state)
                .expect("emit handoff");

        assert_eq!(first_action, RawObserverAction::EmitToPty(request));

        let mut second_output = Vec::new();
        let second_action =
            render_raw_inline_events(&[], &mut second_output, &adapter, "zsh", &mut state)
                .expect("keep handoff foreground protected");

        assert_eq!(second_action, RawObserverAction::RawPassthrough);
    }

    #[test]
    fn pending_shell_handoff_timeout_interrupts_before_preexec_without_notice() {
        let mut state = InlineState::default();
        let request = ShellHandoffRequest::new(
            "sleep 10",
            "$ sleep 10",
            "approved_provider_shell_tool",
            "user",
            "req-timeout-before-preexec",
            "run-timeout-before-preexec",
            1,
        )
        .expect("handoff request");
        state
            .control
            .shell_handoff_mut()
            .enqueue_approved_request(request);
        state
            .control
            .shell_handoff_mut()
            .emit_next_approved()
            .expect("emit handoff");
        state
            .control
            .shell_handoff_mut()
            .backdate_pending_emit_for_test(Duration::from_secs(2));
        let mut output = Vec::new();

        let action = shell_handoff_timeout_recovery_action_with_timeout(
            &mut state,
            false,
            &mut output,
            Some(Duration::from_secs(1)),
        )
        .expect("timeout action");

        assert_eq!(action, Some(RawObserverAction::InterruptForeground));
        assert!(output.is_empty(), "{}", String::from_utf8_lossy(&output));
    }

    #[test]
    fn shell_handoff_timeout_notice_is_deferred_until_foreground_is_idle() {
        let mut state = InlineState::default();
        let request = ShellHandoffRequest::new(
            "sleep 10",
            "$ sleep 10",
            "approved_provider_shell_tool",
            "user",
            "req-timeout",
            "run-timeout",
            1,
        )
        .expect("handoff request");
        state
            .control
            .shell_handoff_mut()
            .enqueue_approved_request(request);
        state
            .control
            .shell_handoff_mut()
            .emit_next_approved()
            .expect("emit handoff");
        state
            .control
            .shell_handoff_mut()
            .backdate_pending_emit_for_test(Duration::from_secs(2));
        let mut busy_output = Vec::new();

        let action = shell_handoff_timeout_recovery_action_with_timeout(
            &mut state,
            true,
            &mut busy_output,
            Some(Duration::from_secs(1)),
        )
        .expect("timeout action");

        assert_eq!(action, Some(RawObserverAction::InterruptForeground));
        assert!(
            busy_output.is_empty(),
            "{}",
            String::from_utf8_lossy(&busy_output)
        );

        state
            .control
            .shell_handoff_mut()
            .pop_pending()
            .expect("handoff finished");
        let mut idle_output = Vec::new();
        let action = shell_handoff_timeout_recovery_action_with_timeout(
            &mut state,
            false,
            &mut idle_output,
            Some(Duration::from_secs(1)),
        )
        .expect("timeout notice");
        let idle_text = String::from_utf8_lossy(&idle_output);

        assert_eq!(action, None);
        assert!(
            idle_text.contains("Command exceeded configured shell handoff timeout (1s)."),
            "{idle_text}"
        );
        assert!(
            idle_text.contains("Sent interrupt to foreground PTY; waiting for shell evidence."),
            "{idle_text}"
        );
    }

    fn test_active_run() -> ActiveAgentRun {
        let request = test_agent_request("active");
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

    fn test_agent_request(id: &str) -> AgentRequest {
        AgentRequest {
            id: id.to_string(),
            session_id: "session-1".to_string(),
            command_block: CommandBlock {
                id: "agent-cmd-1".to_string(),
                session_id: "session-1".to_string(),
                command: "echo test".to_string(),
                origin: Default::default(),
                cwd: "/tmp".to_string(),
                end_cwd: "/tmp".to_string(),
                started_at_ms: 0,
                ended_at_ms: 1,
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
            user_input: Some("test".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }
}
