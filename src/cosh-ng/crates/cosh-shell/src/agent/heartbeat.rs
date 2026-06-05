use std::time::{Duration, Instant};

use crate::agent::run::ActiveAgentRun;
use crate::runtime::prelude::*;

const AGENT_HEARTBEAT_AFTER: Duration = Duration::from_secs(6);
const AGENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) fn render_agent_heartbeat<W: Write>(
    active_run: &mut ActiveAgentRun,
    output: &mut W,
    suppress_for_shell_handoff: bool,
) -> std::io::Result<()> {
    if suppress_for_shell_handoff {
        active_run.status_animation.clear(output)?;
        return Ok(());
    }

    if active_run.has_visible_text_delta {
        return Ok(());
    }

    let i18n = cosh_shell::I18n::new(active_run.language);
    let now = Instant::now();
    if active_run.status_animation.is_enabled() {
        let elapsed = now.duration_since(active_run.started_at).as_secs();
        if elapsed >= AGENT_HEARTBEAT_AFTER.as_secs() {
            let detail = if active_run.current_message.is_empty() {
                active_run.current_phase.as_str()
            } else {
                active_run.current_message.as_str()
            };
            let text = i18n.format(
                cosh_shell::MessageId::AgentThinkingElapsed,
                &[("elapsed", &elapsed.to_string()), ("detail", detail)],
            );
            return active_run.status_animation.render(output, &text);
        }
        return active_run
            .status_animation
            .render(output, i18n.t(cosh_shell::MessageId::AgentThinking));
    }

    if now.duration_since(active_run.started_at) < AGENT_HEARTBEAT_AFTER {
        return Ok(());
    }
    if now.duration_since(active_run.last_activity_at) < AGENT_HEARTBEAT_AFTER {
        return Ok(());
    }
    if now.duration_since(active_run.last_heartbeat_at) < AGENT_HEARTBEAT_INTERVAL {
        return Ok(());
    }

    active_run.last_heartbeat_at = now;
    let elapsed = now.duration_since(active_run.started_at).as_secs_f32();
    let detail = if active_run.current_message.is_empty() {
        active_run.current_phase.as_str()
    } else {
        active_run.current_message.as_str()
    };
    let elapsed_text = format!("{elapsed:.0}");
    writeln!(output)?;
    active_run.renderer.write_notice_panel(
        output,
        NoticePanelModel {
            title: i18n.t(cosh_shell::MessageId::AgentStatusTitle),
            body: vec![i18n.format(
                cosh_shell::MessageId::AgentStillWorking,
                &[("elapsed", &elapsed_text), ("detail", detail)],
            )],
            footer: Some(i18n.t(cosh_shell::MessageId::AgentStatusFooter)),
        },
    )
}

pub(crate) fn remember_agent_activity(active_run: &mut ActiveAgentRun, governed: &[GovernedEvent]) {
    if governed.is_empty() {
        return;
    }

    let i18n = cosh_shell::I18n::new(active_run.language);
    let now = Instant::now();
    active_run.last_activity_at = now;
    for event in governed {
        match &event.event {
            AgentEvent::StatusChanged { phase, message, .. } => {
                active_run.current_phase = phase.clone();
                active_run.current_message = message.clone();
            }
            AgentEvent::TextDelta { .. } => {
                active_run.current_phase = i18n
                    .t(cosh_shell::MessageId::AgentStatusStreaming)
                    .to_string();
                active_run.current_message = i18n
                    .t(cosh_shell::MessageId::AgentStatusReceivingResponse)
                    .to_string();
            }
            AgentEvent::SkillLoadStarted { skill, .. } => {
                active_run.current_phase =
                    i18n.t(cosh_shell::MessageId::AgentStatusSkill).to_string();
                active_run.current_message = i18n.format(
                    cosh_shell::MessageId::AgentStatusLoadingSkill,
                    &[("skill", skill)],
                );
            }
            AgentEvent::ToolCall { name, .. } => {
                active_run.current_phase =
                    i18n.t(cosh_shell::MessageId::AgentStatusTool).to_string();
                active_run.current_message = format!(
                    "{}: {name}",
                    i18n.t(cosh_shell::MessageId::AgentStatusRunningApprovedProviderTool)
                );
            }
            AgentEvent::UserQuestion { question, .. } => {
                active_run.current_phase = i18n
                    .t(cosh_shell::MessageId::AgentStatusQuestion)
                    .to_string();
                active_run.current_message = i18n.format(
                    cosh_shell::MessageId::AgentStatusWaitingUserAnswer,
                    &[("question", question)],
                );
            }
            AgentEvent::Action { command, .. } => {
                active_run.current_phase = i18n
                    .t(cosh_shell::MessageId::AgentStatusApproval)
                    .to_string();
                active_run.current_message = i18n.format(
                    cosh_shell::MessageId::AgentStatusWaitingApprovalCommand,
                    &[("command", command)],
                );
            }
            AgentEvent::ToolPermissionRequest { tool_name, .. } => {
                active_run.current_phase = i18n
                    .t(cosh_shell::MessageId::AgentStatusApproval)
                    .to_string();
                active_run.current_message = i18n.format(
                    cosh_shell::MessageId::AgentStatusWaitingApprovalTool,
                    &[("tool", tool_name)],
                );
            }
            AgentEvent::ToolOutputDelta { tool_id, .. } => {
                active_run.current_phase =
                    i18n.t(cosh_shell::MessageId::AgentStatusTool).to_string();
                active_run.current_message = i18n.format(
                    cosh_shell::MessageId::AgentStatusCapturingToolOutput,
                    &[("tool_id", tool_id)],
                );
            }
            AgentEvent::ToolCompleted {
                tool_id, status, ..
            } => {
                active_run.current_phase =
                    i18n.t(cosh_shell::MessageId::AgentStatusTool).to_string();
                active_run.current_message = i18n.format(
                    cosh_shell::MessageId::AgentStatusToolCompleted,
                    &[("tool_id", tool_id), ("status", status)],
                );
            }
            AgentEvent::AgentCompleted { summary, .. } => {
                active_run.current_phase = i18n
                    .t(cosh_shell::MessageId::AgentStatusCompleted)
                    .to_string();
                active_run.current_message = summary.clone();
            }
            AgentEvent::AgentFailed { error, .. } => {
                active_run.current_phase =
                    i18n.t(cosh_shell::MessageId::AgentStatusFailed).to_string();
                active_run.current_message = error.clone();
            }
            AgentEvent::AgentCancelled { reason, .. } => {
                active_run.current_phase = i18n
                    .t(cosh_shell::MessageId::AgentStatusCancelled)
                    .to_string();
                active_run.current_message = reason.clone();
            }
            AgentEvent::Recommendation { summary, .. }
            | AgentEvent::SkillLoadCompleted { summary, .. } => {
                active_run.current_message = summary.clone();
            }
            AgentEvent::SkillLoadFailed { skill, error, .. } => {
                active_run.current_phase =
                    i18n.t(cosh_shell::MessageId::AgentStatusSkill).to_string();
                active_run.current_message = i18n.format(
                    cosh_shell::MessageId::AgentStatusSkillFailed,
                    &[("skill", skill), ("error", error)],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_active_run() -> ActiveAgentRun {
        let request = AgentRequest {
            id: "request-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: CommandBlock {
                id: "cmd-1".to_string(),
                session_id: "session-1".to_string(),
                command: "hello".to_string(),
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
            user_input: Some("hello".to_string()),
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
            language: cosh_shell::Language::EnUs,
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
        }
    }

    #[test]
    fn tool_call_activity_is_not_reported_as_waiting_for_approval() {
        let mut active_run = test_active_run();
        remember_agent_activity(
            &mut active_run,
            &[GovernedEvent {
                decision: cosh_shell::types::GovernanceDecision::Display,
                policy_decision: cosh_shell::types::GovernancePolicyDecision::NeedsUserApproval,
                event: AgentEvent::ToolCall {
                    run_id: "run-1".to_string(),
                    tool_id: Some("tool-1".to_string()),
                    name: "glob".to_string(),
                    input: r#"{"pattern":"**/README.md"}"#.to_string(),
                },
                reason: "provider tool call visible".to_string(),
                display_text: "provider tool call visible".to_string(),
                auto_execute: false,
            }],
        );

        assert_eq!(active_run.current_phase, "tool");
        assert!(active_run.current_message.contains("provider tool"));
        assert!(!active_run.current_message.contains("approval"));
    }
}
