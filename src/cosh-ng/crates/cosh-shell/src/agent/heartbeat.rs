use std::time::{Duration, Instant};

use crate::agent::run::ActiveAgentRun;
use crate::runtime::prelude::*;

use super::display::display_agent_error;

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

    let i18n = I18n::new(active_run.language);
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
                MessageId::AgentThinkingElapsed,
                &[("elapsed", &elapsed.to_string()), ("detail", detail)],
            );
            return active_run.status_animation.render(output, &text);
        }
        return active_run
            .status_animation
            .render(output, i18n.t(MessageId::AgentThinking));
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
            title: i18n.t(MessageId::AgentStatusTitle),
            body: vec![i18n.format(
                MessageId::AgentStillWorking,
                &[("elapsed", &elapsed_text), ("detail", detail)],
            )],
            footer: Some(i18n.t(MessageId::AgentStatusFooter)),
        },
    )
}

pub(crate) fn remember_agent_activity(active_run: &mut ActiveAgentRun, governed: &[GovernedEvent]) {
    if governed.is_empty() {
        return;
    }

    let i18n = I18n::new(active_run.language);
    let now = Instant::now();
    active_run.last_activity_at = now;
    for event in governed {
        match &event.event {
            AgentEvent::StatusChanged { phase, message, .. } => {
                let (phase, message) = display_status_changed(phase, message, &i18n);
                active_run.current_phase = phase;
                active_run.current_message = message;
            }
            AgentEvent::TextDelta { .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusStreaming).to_string();
                active_run.current_message =
                    i18n.t(MessageId::AgentStatusReceivingResponse).to_string();
            }

            AgentEvent::ToolCall { name, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusTool).to_string();
                active_run.current_message = format!(
                    "{}: {name}",
                    i18n.t(MessageId::AgentStatusRunningApprovedProviderTool)
                );
            }
            AgentEvent::UserQuestion { question, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusQuestion).to_string();
                let question = display_question_text(question, &i18n);
                active_run.current_message = i18n.format(
                    MessageId::AgentStatusWaitingUserAnswer,
                    &[("question", question.as_str())],
                );
            }
            AgentEvent::Action { command, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusApproval).to_string();
                active_run.current_message = i18n.format(
                    MessageId::AgentStatusWaitingApprovalCommand,
                    &[("command", command)],
                );
            }
            AgentEvent::ToolPermissionRequest { tool_name, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusApproval).to_string();
                active_run.current_message = i18n.format(
                    MessageId::AgentStatusWaitingApprovalTool,
                    &[("tool", tool_name)],
                );
            }
            AgentEvent::ToolOutputDelta { tool_id, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusTool).to_string();
                active_run.current_message = i18n.format(
                    MessageId::AgentStatusCapturingToolOutput,
                    &[("tool_id", tool_id)],
                );
            }
            AgentEvent::ToolCompleted {
                tool_id, status, ..
            } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusTool).to_string();
                active_run.current_message = i18n.format(
                    MessageId::AgentStatusToolCompleted,
                    &[("tool_id", tool_id), ("status", status)],
                );
            }
            AgentEvent::AgentCompleted { summary, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusCompleted).to_string();
                active_run.current_message = display_agent_summary(summary, &i18n);
            }
            AgentEvent::AgentFailed { error, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusFailed).to_string();
                active_run.current_message = display_agent_error(error, &i18n);
            }
            AgentEvent::AgentCancelled { reason, .. } => {
                active_run.current_phase = i18n.t(MessageId::AgentStatusCancelled).to_string();
                active_run.current_message = reason.clone();
            }
            AgentEvent::Recommendation { summary, .. } => {
                active_run.current_message = summary.clone();
            }

            AgentEvent::AuthRequired { .. } => {
                active_run.current_phase = "auth".to_string();
                active_run.current_message = "Authentication credentials required".to_string();
            }
            AgentEvent::HookNotification { hook_name, message, .. } => {
                active_run.current_phase = "hook".to_string();
                active_run.current_message = format!("[{hook_name}] {message}");
            }
        }
    }
}

fn display_question_text(question: &str, i18n: &I18n) -> String {
    let question = question.trim();
    if question.is_empty() {
        i18n.t(MessageId::QuestionDefaultPrompt).to_string()
    } else {
        question.to_string()
    }
}

fn display_status_changed(phase: &str, message: &str, i18n: &I18n) -> (String, String) {
    let phase = if phase == "thinking" {
        i18n.t(MessageId::AgentStatusThinking).to_string()
    } else {
        phase.to_string()
    };
    let message = display_status_message(message, i18n);
    (phase, message)
}

fn display_status_message(message: &str, i18n: &I18n) -> String {
    if message == "thinking" {
        return i18n.t(MessageId::AgentStatusThinking).to_string();
    }
    if message == "preparing model session" {
        return i18n
            .t(MessageId::AgentStatusPreparingModelSession)
            .to_string();
    }
    if is_starting_model_backend_message(message) {
        return i18n
            .t(MessageId::AgentStatusStartingModelBackend)
            .to_string();
    }
    if let Some(model) = message.strip_prefix("model initialized ") {
        return i18n.format(MessageId::AgentStatusModelInitialized, &[("model", model)]);
    }
    if let Some(status) = message.strip_prefix("model status: ") {
        return i18n.format(MessageId::AgentStatusModelStatus, &[("status", status)]);
    }
    message.to_string()
}

fn is_starting_model_backend_message(message: &str) -> bool {
    matches!(
        message,
        "Starting model backend"
            | "starting model backend"
            | "starting claude-code stream-json backend"
            | "starting claude-code control protocol backend"
            | "starting co stream-json backend"
            | "starting co control protocol backend"
            | "starting cosh-tui headless backend"
            | "starting cosh-tui control protocol backend"
    )
}

fn display_agent_summary(summary: &str, i18n: &I18n) -> String {
    if summary == "analysis completed" {
        i18n.t(MessageId::AgentStatusAnalysisCompleted).to_string()
    } else {
        summary.to_string()
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
    fn tool_call_activity_is_not_reported_as_waiting_for_approval() {
        let mut active_run = test_active_run();
        remember_agent_activity(
            &mut active_run,
            &[GovernedEvent {
                decision: GovernanceDecision::Display,
                policy_decision: GovernancePolicyDecision::NeedsUserApproval,
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

    #[test]
    fn question_activity_localizes_empty_question_fallback() {
        let mut active_run = test_active_run();
        active_run.language = Language::ZhCn;
        remember_agent_activity(
            &mut active_run,
            &[GovernedEvent {
                decision: GovernanceDecision::Display,
                policy_decision: GovernancePolicyDecision::DisplayOnly,
                event: AgentEvent::UserQuestion {
                    run_id: "run-1".to_string(),
                    provider_request_id: None,
                    question: String::new(),
                    options: Vec::new(),
                    allow_free_text: true,
                    selection_mode: QuestionSelectionMode::Single,
                },
                reason: "agent question requires explicit user input".to_string(),
                display_text: String::new(),
                auto_execute: false,
            }],
        );

        assert_eq!(active_run.current_phase, "问题");
        assert!(active_run.current_message.contains("Agent 需要你的输入"));
        assert!(!active_run
            .current_message
            .contains("Agent needs your input"));
    }

    #[test]
    fn provider_status_activity_localizes_neutral_tokens() {
        let mut active_run = test_active_run();
        active_run.language = Language::ZhCn;
        remember_agent_activity(
            &mut active_run,
            &[GovernedEvent {
                decision: GovernanceDecision::Display,
                policy_decision: GovernancePolicyDecision::DisplayOnly,
                event: AgentEvent::StatusChanged {
                    run_id: "run-1".to_string(),
                    phase: "thinking".to_string(),
                    message: "thinking".to_string(),
                },
                reason: "agent status is display-only".to_string(),
                display_text: String::new(),
                auto_execute: false,
            }],
        );

        assert_eq!(active_run.current_phase, "正在思考");
        assert_eq!(active_run.current_message, "正在思考");
    }

    #[test]
    fn agent_completion_activity_localizes_neutral_summary() {
        let mut active_run = test_active_run();
        active_run.language = Language::ZhCn;
        remember_agent_activity(
            &mut active_run,
            &[GovernedEvent {
                decision: GovernanceDecision::Display,
                policy_decision: GovernancePolicyDecision::DisplayOnly,
                event: AgentEvent::AgentCompleted {
                    run_id: "run-1".to_string(),
                    summary: "analysis completed".to_string(),
                },
                reason: "agent completion is display-only".to_string(),
                display_text: String::new(),
                auto_execute: false,
            }],
        );

        assert_eq!(active_run.current_phase, "已完成");
        assert_eq!(active_run.current_message, "分析完成");
    }

    #[test]
    fn provider_startup_activity_hides_adapter_name_in_zh() {
        let mut active_run = test_active_run();
        active_run.language = Language::ZhCn;
        remember_agent_activity(
            &mut active_run,
            &[GovernedEvent {
                decision: GovernanceDecision::Display,
                policy_decision: GovernancePolicyDecision::DisplayOnly,
                event: AgentEvent::StatusChanged {
                    run_id: "run-1".to_string(),
                    phase: "starting".to_string(),
                    message: "starting cosh-tui headless backend".to_string(),
                },
                reason: "agent status is display-only".to_string(),
                display_text: String::new(),
                auto_execute: false,
            }],
        );

        assert_eq!(active_run.current_message, "正在启动模型后端");
        assert!(!active_run.current_message.contains("cosh-tui"));
    }
}
