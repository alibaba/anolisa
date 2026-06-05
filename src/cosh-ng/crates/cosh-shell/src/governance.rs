use crate::types::{AgentEvent, AuditRecord, GovernanceDecision, GovernedEvent, Policy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceOutput {
    pub events: Vec<GovernedEvent>,
    pub audit: Vec<AuditRecord>,
}

pub fn govern_agent_events(events: &[AgentEvent], policy: &Policy) -> GovernanceOutput {
    let mut governed = Vec::new();
    let mut audit = Vec::new();

    for (idx, event) in events.iter().cloned().enumerate() {
        let (decision, reason, display_text, auto_execute) = match &event {
            AgentEvent::StatusChanged { phase, message, .. } => (
                GovernanceDecision::Display,
                "agent status is display-only".to_string(),
                format!("Status: {phase}\n{message}"),
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
                    reason,
                    format!(
                        "{}{}",
                        summary,
                        render_recommended_commands(commands.as_slice())
                    ),
                    false,
                )
            }
            AgentEvent::ToolCall { name, input, .. } => (
                GovernanceDecision::Rejected,
                "tool calls cannot enter the shell execution path".to_string(),
                render_blocked_tool_request(name, input),
                false,
            ),
            AgentEvent::UserQuestion {
                question, options, ..
            } => (
                GovernanceDecision::Display,
                "agent question requires explicit user input".to_string(),
                render_user_question(question, options),
                false,
            ),
            AgentEvent::Action { command, .. } => (
                GovernanceDecision::Rejected,
                "agent actions cannot execute commands in MVP".to_string(),
                render_blocked_shell_command(command),
                false,
            ),
            AgentEvent::SkillLoadStarted { skill, reason, .. } => (
                GovernanceDecision::Display,
                "skill activity is display-only".to_string(),
                format!("Skill loading: {skill}\nReason: {reason}"),
                false,
            ),
            AgentEvent::SkillLoadCompleted { skill, summary, .. } => (
                GovernanceDecision::Display,
                "skill activity is display-only".to_string(),
                format!("Skill loaded: {skill}\nSummary: {summary}"),
                false,
            ),
            AgentEvent::SkillLoadFailed { skill, error, .. } => (
                GovernanceDecision::Display,
                "skill activity is display-only".to_string(),
                format!("Skill failed: {skill}\nError: {error}"),
                false,
            ),
            AgentEvent::ToolOutputDelta {
                tool_id,
                stream,
                text,
                ..
            } => (
                GovernanceDecision::Display,
                "tool output is display-only".to_string(),
                format!("Tool output: {tool_id} {stream}\n{text}"),
                false,
            ),
            AgentEvent::ToolCompleted {
                tool_id, status, ..
            } => (
                GovernanceDecision::Display,
                "tool completion is display-only".to_string(),
                format!("Tool completed: {tool_id}\nStatus: {status}"),
                false,
            ),
            AgentEvent::TextDelta { text, .. } => (
                GovernanceDecision::Display,
                "assistant text is display-only".to_string(),
                text.clone(),
                false,
            ),
            AgentEvent::AgentCompleted { summary, .. } => (
                GovernanceDecision::Display,
                "agent completion is display-only".to_string(),
                summary.clone(),
                false,
            ),
            AgentEvent::AgentFailed { error, .. } => (
                GovernanceDecision::Display,
                "agent failure is display-only".to_string(),
                error.clone(),
                false,
            ),
            AgentEvent::AgentCancelled { reason, .. } => (
                GovernanceDecision::Display,
                "agent cancellation is display-only".to_string(),
                format!("Agent cancelled\nReason: {reason}"),
                false,
            ),
        };

        let governed_event = GovernedEvent {
            decision: decision.clone(),
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

fn render_recommended_commands(commands: &[String]) -> String {
    if commands.is_empty() {
        return String::new();
    }

    let rendered = commands
        .iter()
        .map(|command| format!("\n  - {command}"))
        .collect::<String>();
    format!("\nrecommended commands:{rendered}")
}

fn render_blocked_tool_request(name: &str, input: &str) -> String {
    format!(
        "Approval required: {}\nCommand: {input}\nBlocked: user approval required",
        user_facing_tool_name(name)
    )
}

fn render_blocked_shell_command(command: &str) -> String {
    format!("Approval required: Shell command\nCommand: {command}\nBlocked: user approval required")
}

fn user_facing_tool_name(name: &str) -> String {
    if name.eq_ignore_ascii_case("bash") || name.eq_ignore_ascii_case("shell") {
        "Bash command".to_string()
    } else {
        format!("{name} tool")
    }
}

fn render_user_question(question: &str, options: &[String]) -> String {
    if options.is_empty() {
        return format!("Question: {question}");
    }

    let rendered = options
        .iter()
        .enumerate()
        .map(|(idx, option)| format!("\n  {}. {}", idx + 1, option))
        .collect::<String>();
    format!("Question: {question}{rendered}")
}
