use crate::command::first_program_token;
use crate::types::{
    AgentMode, AgentRequest, CommandBlock, CommandStatus, Finding, FindingKind, Intervention,
    InterventionDecision, OutputRefs, ShellEvent, ShellEventKind,
};

pub fn findings_from_blocks(blocks: &[CommandBlock]) -> Vec<Finding> {
    let mut findings = Vec::new();

    for block in blocks {
        if block.status == CommandStatus::Failed {
            findings.push(Finding {
                id: format!("finding-{}-nonzero", block.id),
                command_block_id: block.id.clone(),
                kind: FindingKind::NonZeroExit,
                severity: "warning".to_string(),
                message: format!(
                    "command exited with code {}: {}",
                    block.exit_code, block.command
                ),
            });

            if block.exit_code == 127 {
                findings.push(Finding {
                    id: format!("finding-{}-notfound", block.id),
                    command_block_id: block.id.clone(),
                    kind: FindingKind::CommandNotFound,
                    severity: "warning".to_string(),
                    message: "command was not found by the shell".to_string(),
                });
            }

            if block.exit_code == 126 {
                findings.push(Finding {
                    id: format!("finding-{}-permission", block.id),
                    command_block_id: block.id.clone(),
                    kind: FindingKind::PermissionDenied,
                    severity: "warning".to_string(),
                    message: "shell reported permission or executable access failure".to_string(),
                });
            }

            let program = first_program_token(&block.command);
            if program == "systemctl" {
                findings.push(Finding {
                    id: format!("finding-{}-service", block.id),
                    command_block_id: block.id.clone(),
                    kind: FindingKind::ServiceFailed,
                    severity: "warning".to_string(),
                    message: "service command failed and may need service-specific analysis"
                        .to_string(),
                });
            }
        }

        if block.output.terminal_output_ref.is_none() {
            findings.push(Finding {
                id: format!("finding-{}-missing-output", block.id),
                command_block_id: block.id.clone(),
                kind: FindingKind::MissingOutput,
                severity: "info".to_string(),
                message: "command output reference is missing".to_string(),
            });
        }
    }

    findings
}

pub fn interventions_from_findings(findings: &[Finding]) -> Vec<Intervention> {
    findings
        .iter()
        .map(|finding| Intervention {
            id: format!("intervention-{}", finding.id),
            finding_id: finding.id.clone(),
            command_block_id: finding.command_block_id.clone(),
            decision: InterventionDecision::Suggest,
            guidance: guidance_for_finding(&finding.kind),
        })
        .collect()
}

pub fn agent_request_after_confirmation(
    session_id: impl Into<String>,
    block: &CommandBlock,
    findings: &[Finding],
    confirmed: bool,
) -> Option<AgentRequest> {
    if !confirmed {
        return None;
    }

    Some(AgentRequest {
        id: format!("agent-request-{}", block.id),
        session_id: session_id.into(),
        command_block: block.clone(),
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: None,
        findings: findings
            .iter()
            .filter(|finding| finding.command_block_id == block.id)
            .cloned()
            .collect(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    })
}

pub fn agent_request_from_intercepted_input(
    event: &ShellEvent,
    sequence: usize,
    confirmed: bool,
) -> Option<AgentRequest> {
    if !confirmed || event.kind != ShellEventKind::UserInputIntercepted {
        return None;
    }

    let input = event.input.as_ref()?.trim();
    if input.is_empty() {
        return None;
    }

    let started_at_ms = event.started_at_ms.unwrap_or_default();
    let cwd = event
        .cwd
        .clone()
        .filter(|cwd| !cwd.is_empty())
        .unwrap_or_else(|| "<unknown>".to_string());
    let block_id = format!("input-{sequence}");

    Some(AgentRequest {
        id: format!("agent-request-{block_id}"),
        session_id: event.session_id.clone(),
        command_block: CommandBlock {
            id: block_id,
            session_id: event.session_id.clone(),
            command: input.to_string(),
            origin: Default::default(),
            cwd: cwd.clone(),
            end_cwd: cwd,
            started_at_ms,
            ended_at_ms: started_at_ms,
            duration_ms: 0,
            exit_code: 0,
            status: CommandStatus::Completed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        },
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some(input.to_string()),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    })
}

pub fn agent_request_confirmed_by_events(events: &[ShellEvent]) -> bool {
    events.iter().any(event_confirms_failed_command_analysis)
}

pub fn event_requests_agent_cancel(event: &ShellEvent) -> bool {
    if event.kind != ShellEventKind::UserInputIntercepted {
        return false;
    }

    match event.component.as_deref() {
        Some("slash") | None => matches_agent_cancel_slash(event.input.as_deref()),
        Some("control") => event.input.as_deref() == Some("ctrl_c"),
        Some("card") => event.message.as_deref() == Some("agent_cancel"),
        _ => false,
    }
}

pub fn event_confirms_failed_command_analysis(event: &ShellEvent) -> bool {
    if event.kind != ShellEventKind::UserInputIntercepted {
        return false;
    }

    match event.component.as_deref() {
        Some("slash") => matches_failure_analysis_slash(event.input.as_deref()),
        Some("card") => event.message.as_deref() == Some("failed_command_analyze"),
        None => matches_failure_analysis_slash(event.input.as_deref()),
        _ => false,
    }
}

pub fn event_cancels_failed_command_analysis(event: &ShellEvent) -> bool {
    if event.kind != ShellEventKind::UserInputIntercepted {
        return false;
    }

    match event.component.as_deref() {
        Some("slash") => matches_cancel_slash(event.input.as_deref()),
        Some("card") => event.message.as_deref() == Some("failed_command_dismiss"),
        None => matches_cancel_slash(event.input.as_deref()),
        _ => false,
    }
}

pub fn recommendation_selection_from_event(event: &ShellEvent) -> Option<usize> {
    recommendation_action_from_event(event).map(|action| action.index)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalCommandKind {
    Approve,
    AlwaysTrust,
    Deny,
    Details,
    SendToShell,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCommand {
    pub kind: ApprovalCommandKind,
    pub id: String,
}

pub fn approval_command_from_event(event: &ShellEvent) -> Option<ApprovalCommand> {
    if event.kind != ShellEventKind::UserInputIntercepted {
        return None;
    }

    if event.component.as_deref() == Some("card") {
        let id = event.input.as_deref()?.trim();
        if id.is_empty() {
            return None;
        }
        if id.starts_with("consultation-") {
            return None;
        }
        return match event.message.as_deref() {
            Some("approve") => Some(ApprovalCommand {
                kind: ApprovalCommandKind::Approve,
                id: id.to_string(),
            }),
            Some("always_trust") => Some(ApprovalCommand {
                kind: ApprovalCommandKind::AlwaysTrust,
                id: id.to_string(),
            }),
            Some("deny") => Some(ApprovalCommand {
                kind: ApprovalCommandKind::Deny,
                id: id.to_string(),
            }),
            Some("details") => Some(ApprovalCommand {
                kind: ApprovalCommandKind::Details,
                id: id.to_string(),
            }),
            Some("send_to_shell") => Some(ApprovalCommand {
                kind: ApprovalCommandKind::SendToShell,
                id: id.to_string(),
            }),
            Some("cancel") => Some(ApprovalCommand {
                kind: ApprovalCommandKind::Cancel,
                id: id.to_string(),
            }),
            _ => None,
        };
    }

    parse_approval_details_command(event)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendationActionKind {
    Select,
    Copy,
    Insert,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecommendationAction {
    pub kind: RecommendationActionKind,
    pub index: usize,
}

pub fn recommendation_action_from_event(event: &ShellEvent) -> Option<RecommendationAction> {
    if event.kind != ShellEventKind::UserInputIntercepted {
        return None;
    }

    match event.component.as_deref() {
        Some("slash") | None => parse_recommendation_action(event.input.as_deref()),
        Some("card") => parse_recommendation_card_action(event),
        _ => None,
    }
}

fn matches_failure_analysis_slash(input: Option<&str>) -> bool {
    let first_token = input
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default();
    matches!(first_token, "/explain" | "/agent")
}

fn matches_cancel_slash(input: Option<&str>) -> bool {
    let first_token = input
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default();
    matches!(first_token, "/cancel" | "/clear" | "/shell")
}

fn matches_agent_cancel_slash(input: Option<&str>) -> bool {
    let first_token = input
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default();
    first_token == "/cancel"
}

fn parse_recommendation_action(input: Option<&str>) -> Option<RecommendationAction> {
    let mut tokens = input.unwrap_or_default().split_whitespace();
    let command = tokens.next()?;
    let kind = match command {
        "/select" => RecommendationActionKind::Select,
        "/copy" => RecommendationActionKind::Copy,
        _ => return None,
    };

    let index = tokens
        .next()?
        .parse::<usize>()
        .ok()
        .filter(|index| *index > 0)?;
    Some(RecommendationAction { kind, index })
}

fn parse_recommendation_card_action(event: &ShellEvent) -> Option<RecommendationAction> {
    let kind = match event.message.as_deref()? {
        "recommendation_copy" => RecommendationActionKind::Copy,
        "recommendation_insert" => RecommendationActionKind::Insert,
        "recommendation_details" => RecommendationActionKind::Details,
        _ => return None,
    };
    let index = event
        .input
        .as_deref()?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|index| *index > 0)?;
    Some(RecommendationAction { kind, index })
}

fn parse_approval_details_command(event: &ShellEvent) -> Option<ApprovalCommand> {
    match event.component.as_deref() {
        Some("slash") | None => {}
        _ => return None,
    }

    let input = event.input.as_deref();
    let mut tokens = input.unwrap_or_default().split_whitespace();
    let command = tokens.next()?;
    let id = tokens.next()?.to_string();
    let kind = match command {
        "/details" => ApprovalCommandKind::Details,
        "/send-to-shell" => ApprovalCommandKind::SendToShell,
        _ => return None,
    };

    Some(ApprovalCommand { kind, id })
}

fn guidance_for_finding(kind: &FindingKind) -> String {
    match kind {
        FindingKind::NonZeroExit => {
            "show a short explanation and ask before deeper Agent analysis".to_string()
        }
        FindingKind::CommandNotFound => {
            "recommend checking PATH, package availability, or command spelling".to_string()
        }
        FindingKind::PermissionDenied => {
            "recommend checking executable bit, ownership, or required privileges".to_string()
        }
        FindingKind::ServiceFailed => {
            "recommend collecting service status and recent logs".to_string()
        }
        FindingKind::MissingOutput => {
            "recommend retrying with output capture enabled before detailed analysis".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        approval_command_from_event, event_requests_agent_cancel, recommendation_action_from_event,
        ApprovalCommand, ApprovalCommandKind, RecommendationAction, RecommendationActionKind,
    };
    use crate::types::ShellEvent;

    #[test]
    fn parses_recommendation_actions_from_slash_events() {
        let mut allow = ShellEvent::user_input_intercepted("session-1", "/allow 2");
        allow.component = Some("slash".to_string());
        let mut select = ShellEvent::user_input_intercepted("session-1", "/select 1");
        select.component = Some("slash".to_string());
        let mut copy = ShellEvent::user_input_intercepted("session-1", "/copy 1");
        copy.component = Some("slash".to_string());
        let mut approve = ShellEvent::user_input_intercepted("session-1", "/approve 2");
        approve.component = Some("slash".to_string());
        let mut deny = ShellEvent::user_input_intercepted("session-1", "/deny 2");
        deny.component = Some("slash".to_string());

        assert_eq!(recommendation_action_from_event(&allow), None);
        assert_eq!(
            recommendation_action_from_event(&select),
            Some(RecommendationAction {
                kind: RecommendationActionKind::Select,
                index: 1,
            })
        );
        assert_eq!(
            recommendation_action_from_event(&copy),
            Some(RecommendationAction {
                kind: RecommendationActionKind::Copy,
                index: 1,
            })
        );
        assert_eq!(recommendation_action_from_event(&approve), None);
        assert_eq!(recommendation_action_from_event(&deny), None);
    }

    #[test]
    fn parses_recommendation_actions_from_card_events() {
        let mut copy = ShellEvent::user_input_intercepted("session-1", "2");
        copy.component = Some("card".to_string());
        copy.message = Some("recommendation_copy".to_string());
        let mut insert = ShellEvent::user_input_intercepted("session-1", "3");
        insert.component = Some("card".to_string());
        insert.message = Some("recommendation_insert".to_string());
        let mut details = ShellEvent::user_input_intercepted("session-1", "1");
        details.component = Some("card".to_string());
        details.message = Some("recommendation_details".to_string());

        assert_eq!(
            recommendation_action_from_event(&copy),
            Some(RecommendationAction {
                kind: RecommendationActionKind::Copy,
                index: 2,
            })
        );
        assert_eq!(
            recommendation_action_from_event(&insert),
            Some(RecommendationAction {
                kind: RecommendationActionKind::Insert,
                index: 3,
            })
        );
        assert_eq!(
            recommendation_action_from_event(&details),
            Some(RecommendationAction {
                kind: RecommendationActionKind::Details,
                index: 1,
            })
        );
    }

    #[test]
    fn parses_approval_commands_from_card_events() {
        let mut approve = ShellEvent::user_input_intercepted("session-1", "req-1");
        approve.component = Some("card".to_string());
        approve.message = Some("approve".to_string());
        let mut deny = ShellEvent::user_input_intercepted("session-1", "req-3");
        deny.component = Some("card".to_string());
        deny.message = Some("deny".to_string());
        let mut details = ShellEvent::user_input_intercepted("session-1", "/details req-4");
        details.component = Some("slash".to_string());
        let mut cancel = ShellEvent::user_input_intercepted("session-1", "req-5");
        cancel.component = Some("card".to_string());
        cancel.message = Some("cancel".to_string());
        let mut send_to_shell = ShellEvent::user_input_intercepted("session-1", "handoff-1");
        send_to_shell.component = Some("card".to_string());
        send_to_shell.message = Some("send_to_shell".to_string());
        let mut recommendation = ShellEvent::user_input_intercepted("session-1", "/approve 2");
        recommendation.component = Some("slash".to_string());

        assert_eq!(
            approval_command_from_event(&approve),
            Some(ApprovalCommand {
                kind: ApprovalCommandKind::Approve,
                id: "req-1".to_string(),
            })
        );
        assert_eq!(
            approval_command_from_event(&deny),
            Some(ApprovalCommand {
                kind: ApprovalCommandKind::Deny,
                id: "req-3".to_string(),
            })
        );
        assert_eq!(
            approval_command_from_event(&details),
            Some(ApprovalCommand {
                kind: ApprovalCommandKind::Details,
                id: "req-4".to_string(),
            })
        );
        assert_eq!(
            approval_command_from_event(&cancel),
            Some(ApprovalCommand {
                kind: ApprovalCommandKind::Cancel,
                id: "req-5".to_string(),
            })
        );
        assert_eq!(
            approval_command_from_event(&send_to_shell),
            Some(ApprovalCommand {
                kind: ApprovalCommandKind::SendToShell,
                id: "handoff-1".to_string(),
            })
        );
        assert_eq!(approval_command_from_event(&recommendation), None);
    }

    #[test]
    fn consultation_card_events_are_not_approval_commands() {
        let mut analyze =
            ShellEvent::user_input_intercepted("session-1", "consultation-hook-cmd-3");
        analyze.component = Some("card".to_string());
        analyze.message = Some("approve".to_string());

        assert_eq!(approval_command_from_event(&analyze), None);
    }

    #[test]
    fn parses_agent_cancel_slash_event() {
        let mut cancel = ShellEvent::user_input_intercepted("session-1", "/cancel");
        cancel.component = Some("slash".to_string());
        let mut ctrl_c = ShellEvent::user_input_intercepted("session-1", "ctrl_c");
        ctrl_c.component = Some("control".to_string());
        let mut agent_cancel = ShellEvent::user_input_intercepted("session-1", "agent-request-1");
        agent_cancel.component = Some("card".to_string());
        agent_cancel.message = Some("agent_cancel".to_string());
        let mut approval_cancel = ShellEvent::user_input_intercepted("session-1", "req-1");
        approval_cancel.component = Some("card".to_string());
        approval_cancel.message = Some("cancel".to_string());
        let mut clear = ShellEvent::user_input_intercepted("session-1", "/clear");
        clear.component = Some("slash".to_string());

        assert!(event_requests_agent_cancel(&cancel));
        assert!(event_requests_agent_cancel(&ctrl_c));
        assert!(event_requests_agent_cancel(&agent_cancel));
        assert!(!event_requests_agent_cancel(&approval_cancel));
        assert!(!event_requests_agent_cancel(&clear));
    }
}
