use crate::approval::approved_tool::request_is_executable_bash_tool;
use crate::approval::broker::{
    approval_execution_metadata, classify_approval_outcome, ApprovalExecutionMetadata,
    ApprovalOutcome, ApprovalOutcomeInput,
};
use crate::approval::handoff::{
    fallback_bash_execution_path, shell_handoff_command_from_request, trust_key_from_command,
    ApprovedBashExecutionPath,
};
use crate::approval::journal::approval_journal_entry;
use crate::approval::provider::provider_approval_status;
use crate::runtime::prelude::*;

pub(crate) struct AppliedApprovalDecision {
    pub(crate) request: RuntimeApprovalRequest,
    pub(crate) title: MessageId,
    pub(crate) run_approved_tool: bool,
}

pub(crate) fn apply_approval_decision(
    state: &mut InlineState,
    request_index: usize,
    kind: ApprovalCommandKind,
) -> Option<AppliedApprovalDecision> {
    let (status, title) = match kind {
        ApprovalCommandKind::Approve => {
            approval_status_for_allowed_request(&state.approvals.requests[request_index])
        }
        ApprovalCommandKind::AlwaysTrust => {
            let (status, _) =
                approval_status_for_allowed_request(&state.approvals.requests[request_index]);
            if let Some(key) =
                trust_key_from_command(&state.approvals.requests[request_index].preview)
            {
                state.control.trust_session_command(key);
            }
            (status, MessageId::ApprovalResolutionTrustedTitle)
        }
        ApprovalCommandKind::Deny => (
            ApprovalRequestStatus::Denied,
            MessageId::ApprovalResolutionDeniedTitle,
        ),
        ApprovalCommandKind::Cancel => (
            ApprovalRequestStatus::Cancelled,
            MessageId::ApprovalResolutionCancelledTitle,
        ),
        ApprovalCommandKind::Details => return None,
        ApprovalCommandKind::SendToShell => return None,
    };

    state.approvals.requests[request_index].status = status;
    let outcome = approval_outcome_for_request(state, &state.approvals.requests[request_index]);
    let metadata = approval_execution_metadata(
        outcome,
        provider_approval_status(status),
        request_is_executable_bash_tool(&state.approvals.requests[request_index]),
    );
    apply_approval_execution_metadata(&mut state.approvals.requests[request_index], metadata);
    let request = state.approvals.requests[request_index].clone();
    state
        .approvals
        .journal
        .push(approval_journal_entry(&request, "user"));
    let run_approved_tool =
        status == ApprovalRequestStatus::Approved && request_is_executable_bash_tool(&request);

    Some(AppliedApprovalDecision {
        request,
        title,
        run_approved_tool,
    })
}

fn apply_approval_execution_metadata(
    request: &mut RuntimeApprovalRequest,
    metadata: ApprovalExecutionMetadata,
) {
    request.execution_path = metadata.execution_path;
    request.redaction_status = metadata.redaction_status;
}

fn approval_status_for_allowed_request(
    request: &RuntimeApprovalRequest,
) -> (ApprovalRequestStatus, MessageId) {
    if request_is_executable_bash_tool(request) {
        let command = match shell_handoff_command_from_request(request) {
            Ok(command) => command,
            Err(_) => {
                return (
                    ApprovalRequestStatus::Blocked,
                    MessageId::ApprovalResolutionBlockedTitle,
                )
            }
        };
        if fallback_bash_execution_path(&command) == ApprovedBashExecutionPath::Blocked {
            return (
                ApprovalRequestStatus::Blocked,
                MessageId::ApprovalResolutionBlockedTitle,
            );
        }
    }

    (
        ApprovalRequestStatus::Approved,
        MessageId::ApprovalResolutionApprovedTitle,
    )
}

pub(crate) fn active_provider_supports_host_executed_shell(state: &InlineState) -> bool {
    state.agent_run.active.as_ref().is_some_and(|run| {
        run.handle
            .control_capabilities()
            .can_handle_host_executed_shell_tool_result
    })
}

pub(crate) fn request_can_receive_host_executed_result(
    state: &InlineState,
    request: &RuntimeApprovalRequest,
) -> bool {
    request_is_executable_bash_tool(request)
        && request.provider_shell_request_kind.is_control_permission()
        && request.request_id.is_some()
        && request.tool_use_id.is_some()
        && active_provider_supports_host_executed_shell(state)
}

pub(crate) fn approval_outcome_for_request(
    _state: &InlineState,
    request: &RuntimeApprovalRequest,
) -> ApprovalOutcome {
    classify_approval_outcome(ApprovalOutcomeInput {
        approved: request.status == ApprovalRequestStatus::Approved,
        shell_tool: request_is_executable_bash_tool(request),
        provider_request: request.provider_shell_request_kind.is_control_permission(),
    })
}

pub(crate) fn should_send_approval_resolution_to_agent(
    state: &InlineState,
    request: &RuntimeApprovalRequest,
) -> bool {
    matches!(
        request.status,
        ApprovalRequestStatus::Denied | ApprovalRequestStatus::Cancelled
    ) && !state
        .approvals
        .requests
        .iter()
        .any(|request| request.status == ApprovalRequestStatus::Pending)
}

pub(crate) fn approval_resolution_agent_request(request: &RuntimeApprovalRequest) -> AgentRequest {
    let decision = match request.status {
        ApprovalRequestStatus::Denied => "denied by user",
        ApprovalRequestStatus::Cancelled => "cancelled by user",
        ApprovalRequestStatus::Blocked => "blocked by cosh-shell",
        ApprovalRequestStatus::Pending => "pending",
        ApprovalRequestStatus::Approved => "approved",
    };
    let block_id = format!("approval-resolution-{}", request.id);
    let user_input = format!(
        "Approval result for request {id}\n\
         Tool: {subject}\n\
         Command: {command}\n\
         Decision: {decision}\n\
         Status: not_executed\n\
         No command ran.\n\
         Continue the same Agent session using this approval result. Do not claim the command executed. Provide a safe next step or ask for another approval if more evidence is required.",
        id = request.id,
        subject = request.subject,
        command = request.preview,
        decision = decision,
    );

    AgentRequest {
        id: format!("agent-request-{block_id}"),
        session_id: request.session_id.clone(),
        command_block: CommandBlock {
            id: block_id,
            session_id: request.session_id.clone(),
            command: user_input.clone(),
            origin: Default::default(),
            cwd: request.cwd.clone(),
            end_cwd: request.cwd.clone(),
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        },
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some(user_input),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_without_active_host_executed_provider_is_not_deliverable() {
        let state = InlineState::default();
        let request = shell_permission_request(Some("ctrl-1"), Some("toolu-1"));

        assert!(!request_can_receive_host_executed_result(&state, &request));
        assert_eq!(
            approval_outcome_for_request(&state, &request),
            ApprovalOutcome::ForegroundShellHandoff
        );
    }

    #[test]
    fn request_missing_control_ids_is_not_host_executed_deliverable() {
        let state = InlineState::default();

        for request in [
            shell_permission_request(None, Some("toolu-1")),
            shell_permission_request(Some("ctrl-1"), None),
        ] {
            assert!(!request_can_receive_host_executed_result(&state, &request));
        }
    }

    #[test]
    fn streamed_tool_fallback_with_ids_is_not_provider_control_owned() {
        let state = InlineState::default();
        let mut request = shell_permission_request(Some("ctrl-1"), Some("toolu-1"));
        request.provider_shell_request_kind = ProviderShellRequestKind::StreamedToolCallFallback;

        assert!(!request_can_receive_host_executed_result(&state, &request));
        assert_eq!(
            approval_outcome_for_request(&state, &request),
            ApprovalOutcome::ForegroundShellHandoff
        );
    }

    #[test]
    fn approval_resolution_request_marks_command_not_executed() {
        for (status, decision) in [
            (ApprovalRequestStatus::Denied, "denied by user"),
            (ApprovalRequestStatus::Cancelled, "cancelled by user"),
            (ApprovalRequestStatus::Blocked, "blocked by cosh-shell"),
        ] {
            let mut request = shell_permission_request(Some("ctrl-1"), Some("toolu-1"));
            request.status = status;

            let agent_request = approval_resolution_agent_request(&request);
            let input = agent_request.user_input.expect("approval result input");

            assert!(input.contains(&format!("Decision: {decision}")), "{input}");
            assert!(input.contains("Status: not_executed"), "{input}");
            assert!(input.contains("No command ran."), "{input}");
            assert_eq!(agent_request.command_block.output.terminal_output_ref, None);
            assert_eq!(agent_request.command_block.output.terminal_output_bytes, 0);
        }
    }

    fn shell_permission_request(
        request_id: Option<&str>,
        tool_use_id: Option<&str>,
    ) -> RuntimeApprovalRequest {
        RuntimeApprovalRequest {
            id: "req-1".to_string(),
            run_id: "run-1".to_string(),
            session_id: "sess-1".to_string(),
            cwd: "/tmp".to_string(),
            source: "control-protocol",
            provider_shell_request_kind: if request_id.is_some() && tool_use_id.is_some() {
                ProviderShellRequestKind::ControlPermission
            } else {
                ProviderShellRequestKind::StreamedToolCallFallback
            },
            kind: ApprovalRequestKind::Tool,
            subject: "run_shell_command".to_string(),
            preview: "$ echo ok".to_string(),
            risk: "medium",
            request_id: request_id.map(str::to_string),
            tool_use_id: tool_use_id.map(str::to_string),
            tool_input: Some(serde_json::json!({ "command": "echo ok" })),
            original_user_request: None,
            status: ApprovalRequestStatus::Approved,
            execution_path: Some("provider_control_protocol"),
            command_block_id: None,
            redaction_status: None,
            assessment: None,
        }
    }
}
