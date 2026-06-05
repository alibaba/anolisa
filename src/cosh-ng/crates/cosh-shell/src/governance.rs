use crate::{
    config::Language,
    i18n::{I18n, MessageId},
    types::{
        AgentEvent, AuditRecord, GovernanceDecision, GovernancePolicyDecision, GovernedEvent,
        Policy,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceOutput {
    pub events: Vec<GovernedEvent>,
    pub audit: Vec<AuditRecord>,
}

pub fn govern_agent_events(events: &[AgentEvent], policy: &Policy) -> GovernanceOutput {
    govern_agent_events_with_language(events, policy, Language::EnUs)
}

pub fn govern_agent_events_with_language(
    events: &[AgentEvent],
    policy: &Policy,
    language: Language,
) -> GovernanceOutput {
    let i18n = I18n::new(language);
    let mut governed = Vec::new();
    let mut audit = Vec::new();

    for (idx, event) in events.iter().cloned().enumerate() {
        let (decision, policy_decision, reason, display_text, auto_execute) = match &event {
            AgentEvent::StatusChanged { phase, message, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "agent status is display-only".to_string(),
                format!(
                    "{}\n{message}",
                    i18n.format(MessageId::AgentGovernanceStatusLine, &[("phase", phase)])
                ),
                false,
            ),
            AgentEvent::Recommendation {
                summary,
                commands,
                auto_execute,
                ..
            } => {
                let stripped = *auto_execute || policy.recommend_only;
                let reason = if stripped {
                    "recommendation is display-only in MVP".to_string()
                } else {
                    "recommendation allowed for display".to_string()
                };
                (
                    if stripped {
                        GovernanceDecision::Degraded
                    } else {
                        GovernanceDecision::Display
                    },
                    GovernancePolicyDecision::DisplayOnly,
                    reason,
                    format!(
                        "{}{}",
                        summary,
                        render_recommended_commands(commands.as_slice(), &i18n)
                    ),
                    false,
                )
            }
            AgentEvent::ToolCall { name, input, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::NeedsUserApproval,
                "tool call requires explicit approval before execution".to_string(),
                render_blocked_tool_request(name, input, &i18n),
                false,
            ),
            AgentEvent::UserQuestion {
                question, options, ..
            } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "agent question requires explicit user input".to_string(),
                render_user_question(question, options, &i18n),
                false,
            ),
            AgentEvent::Action { command, .. } => (
                GovernanceDecision::Rejected,
                GovernancePolicyDecision::HostBlocked,
                "agent actions cannot execute commands in MVP".to_string(),
                render_blocked_shell_command(command, &i18n),
                false,
            ),
            AgentEvent::ToolPermissionRequest {
                tool_name,
                tool_input,
                ..
            } => {
                let input_str = serde_json::to_string(tool_input).unwrap_or_default();
                (
                    GovernanceDecision::Display,
                    GovernancePolicyDecision::NeedsUserApproval,
                    "tool permission request via control protocol".to_string(),
                    render_blocked_tool_request(tool_name, &input_str, &i18n),
                    false,
                )
            }
            AgentEvent::SkillLoadStarted { skill, reason, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "skill activity is display-only".to_string(),
                format!(
                    "{}\n{}",
                    i18n.format(
                        MessageId::AgentGovernanceSkillLoadingLine,
                        &[("skill", skill)]
                    ),
                    i18n.format(MessageId::AgentGovernanceReasonLine, &[("reason", reason)])
                ),
                false,
            ),
            AgentEvent::SkillLoadCompleted { skill, summary, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "skill activity is display-only".to_string(),
                format!(
                    "{}\n{}",
                    i18n.format(
                        MessageId::AgentGovernanceSkillLoadedLine,
                        &[("skill", skill)]
                    ),
                    i18n.format(
                        MessageId::AgentGovernanceSummaryLine,
                        &[("summary", summary)]
                    )
                ),
                false,
            ),
            AgentEvent::SkillLoadFailed { skill, error, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "skill activity is display-only".to_string(),
                format!(
                    "{}\n{}",
                    i18n.format(
                        MessageId::AgentGovernanceSkillFailedLine,
                        &[("skill", skill)]
                    ),
                    i18n.format(MessageId::AgentGovernanceErrorLine, &[("error", error)])
                ),
                false,
            ),
            AgentEvent::ToolOutputDelta {
                tool_id,
                stream,
                text,
                ..
            } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::AuditOnly,
                "tool output is display-only".to_string(),
                format!(
                    "{}\n{text}",
                    i18n.format(
                        MessageId::AgentGovernanceToolOutputLine,
                        &[("tool_id", tool_id), ("stream", stream)]
                    )
                ),
                false,
            ),
            AgentEvent::ToolCompleted {
                tool_id, status, ..
            } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::AuditOnly,
                "tool completion is display-only".to_string(),
                format!(
                    "{}\n{}",
                    i18n.format(
                        MessageId::AgentGovernanceToolCompletedLine,
                        &[("tool_id", tool_id)]
                    ),
                    i18n.format(MessageId::AgentGovernanceStatusLine, &[("phase", status)])
                ),
                false,
            ),
            AgentEvent::TextDelta { text, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "assistant text is display-only".to_string(),
                text.clone(),
                false,
            ),
            AgentEvent::AgentCompleted { summary, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "agent completion is display-only".to_string(),
                summary.clone(),
                false,
            ),
            AgentEvent::AgentFailed { error, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "agent failure is display-only".to_string(),
                error.clone(),
                false,
            ),
            AgentEvent::AgentCancelled { reason, .. } => (
                GovernanceDecision::Display,
                GovernancePolicyDecision::DisplayOnly,
                "agent cancellation is display-only".to_string(),
                format!(
                    "{}\n{}",
                    i18n.t(MessageId::FailedAnalysisCancelledTitle),
                    i18n.format(
                        MessageId::AgentGovernanceReasonLine,
                        &[("reason", &agent_cancelled_reason(reason, &i18n))]
                    )
                ),
                false,
            ),
        };

        let governed_event = GovernedEvent {
            decision: decision.clone(),
            policy_decision,
            event,
            reason: reason.clone(),
            display_text,
            auto_execute,
        };

        audit.push(AuditRecord {
            id: format!("audit-{idx}"),
            subject: format!("{:?}", governed_event.event),
            decision,
            reason,
        });
        governed.push(governed_event);
    }

    GovernanceOutput {
        events: governed,
        audit,
    }
}

fn render_recommended_commands(commands: &[String], i18n: &I18n) -> String {
    if commands.is_empty() {
        return String::new();
    }

    let rendered = commands
        .iter()
        .map(|command| format!("\n  - {command}"))
        .collect::<String>();
    format!(
        "\n{}{rendered}",
        i18n.t(MessageId::AgentRecommendedCommandsLabel)
    )
}

fn render_blocked_tool_request(name: &str, input: &str, i18n: &I18n) -> String {
    format!(
        "{}\n{}: {input}\n{}",
        i18n.format(
            MessageId::AgentGovernanceApprovalRequiredLine,
            &[("subject", &user_facing_tool_name(name, i18n))]
        ),
        i18n.t(MessageId::ApprovalCommandLabel),
        i18n.t(MessageId::AgentGovernanceBlockedUserApprovalLine)
    )
}

fn render_blocked_shell_command(command: &str, i18n: &I18n) -> String {
    format!(
        "{}\n{}: {command}\n{}",
        i18n.format(
            MessageId::AgentGovernanceApprovalRequiredLine,
            &[(
                "subject",
                i18n.t(MessageId::AgentGovernanceShellCommandSubject)
            )]
        ),
        i18n.t(MessageId::ApprovalCommandLabel),
        i18n.t(MessageId::AgentGovernanceBlockedUserApprovalLine)
    )
}

fn user_facing_tool_name(name: &str, i18n: &I18n) -> String {
    if name.eq_ignore_ascii_case("bash") || name.eq_ignore_ascii_case("shell") {
        i18n.t(MessageId::AgentGovernanceBashCommandSubject)
            .to_string()
    } else {
        i18n.format(MessageId::AgentGovernanceToolSubject, &[("tool", name)])
    }
}

fn render_user_question(question: &str, options: &[String], i18n: &I18n) -> String {
    if options.is_empty() {
        return i18n.format(
            MessageId::AgentGovernanceQuestionLine,
            &[("question", question)],
        );
    }

    let rendered = options
        .iter()
        .enumerate()
        .map(|(idx, option)| format!("\n  {}. {}", idx + 1, option))
        .collect::<String>();
    format!(
        "{}{rendered}",
        i18n.format(
            MessageId::AgentGovernanceQuestionLine,
            &[("question", question)]
        )
    )
}

fn agent_cancelled_reason(reason: &str, i18n: &I18n) -> String {
    if reason == "user requested cancellation" {
        return i18n
            .t(MessageId::AgentCancelledUserRequestedReason)
            .to_string();
    }
    reason.to_string()
}
