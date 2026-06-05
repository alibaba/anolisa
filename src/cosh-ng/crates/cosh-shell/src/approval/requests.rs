use crate::approval::journal::approval_journal_entry;
use crate::runtime::prelude::*;
use cosh_shell::tools::display::{display_for_tool, ToolColor};
use cosh_shell::tools::is_readonly_builtin_tool_name;
use cosh_shell::types::GovernancePolicyDecision;

pub(crate) fn record_approval_requests(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    run_request: Option<&AgentRequest>,
    ignore_tool_calls: bool,
) -> Vec<String> {
    let mut ids = Vec::new();
    let session_id = run_request
        .map(|request| request.session_id.clone())
        .unwrap_or_else(|| "unknown-session".to_string());
    let cwd = run_request
        .map(|request| request.command_block.cwd.clone())
        .unwrap_or_else(|| "<unknown>".to_string());
    let original_user_request = original_user_request(run_request);
    for event in governed_events {
        let request = approval_request_from_event(
            state,
            event,
            &session_id,
            &cwd,
            original_user_request.as_deref(),
            ignore_tool_calls,
        );

        if let Some(request) = request {
            if state
                .approvals
                .requests
                .iter()
                .any(|existing| same_approval_request_identity(existing, &request))
            {
                continue;
            }
            ids.push(request.id.clone());
            state.approvals.requests.push(request);
        }
    }
    ids
}

fn same_approval_request_identity(
    existing: &RuntimeApprovalRequest,
    request: &RuntimeApprovalRequest,
) -> bool {
    if existing.run_id != request.run_id {
        return false;
    }
    if let (Some(existing_id), Some(request_id)) = (&existing.tool_use_id, &request.tool_use_id) {
        return existing_id == request_id;
    }
    match (&existing.request_id, &request.request_id) {
        (Some(existing_id), Some(request_id)) => existing_id == request_id,
        _ => {
            existing.kind == request.kind
                && existing.subject == request.subject
                && existing.preview == request.preview
        }
    }
}

pub(crate) fn approval_request_from_governed_event(
    state: &InlineState,
    event: &GovernedEvent,
    run_request: Option<&AgentRequest>,
    ignore_tool_calls: bool,
) -> Option<RuntimeApprovalRequest> {
    let session_id = run_request
        .map(|request| request.session_id.clone())
        .unwrap_or_else(|| "unknown-session".to_string());
    let cwd = run_request
        .map(|request| request.command_block.cwd.clone())
        .unwrap_or_else(|| "<unknown>".to_string());
    let original_user_request = original_user_request(run_request);
    approval_request_from_event(
        state,
        event,
        &session_id,
        &cwd,
        original_user_request.as_deref(),
        ignore_tool_calls,
    )
}

fn approval_request_from_event(
    state: &InlineState,
    event: &GovernedEvent,
    session_id: &str,
    cwd: &str,
    original_user_request: Option<&str>,
    ignore_tool_calls: bool,
) -> Option<RuntimeApprovalRequest> {
    if event.policy_decision != GovernancePolicyDecision::NeedsUserApproval {
        return None;
    }

    match &event.event {
        AgentEvent::ToolCall {
            run_id,
            tool_id,
            name,
            input,
        } => {
            if ignore_tool_calls {
                return None;
            }
            let info = display_for_tool(name, input);
            if is_readonly_builtin_tool_name(&info.label) {
                return None;
            }
            let risk = match info.color {
                ToolColor::Dangerous => "high",
                _ => "medium",
            };
            Some(RuntimeApprovalRequest {
                id: next_approval_id(state),
                run_id: run_id.clone(),
                session_id: session_id.to_string(),
                cwd: cwd.to_string(),
                source: "agent",
                provider_shell_request_kind: ProviderShellRequestKind::StreamedToolCallFallback,
                kind: ApprovalRequestKind::Tool,
                subject: info.label,
                preview: info.preview,
                risk,
                request_id: None,
                tool_use_id: tool_id.clone(),
                tool_input: None,
                original_user_request: original_user_request.map(ToString::to_string),
                status: ApprovalRequestStatus::Pending,
                execution_path: None,
                command_block_id: None,
                redaction_status: None,
            })
        }
        AgentEvent::Action { run_id, command } => Some(RuntimeApprovalRequest {
            id: next_approval_id(state),
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            source: "agent",
            provider_shell_request_kind: ProviderShellRequestKind::LocalApproval,
            kind: ApprovalRequestKind::ShellCommand,
            subject: "shell command".to_string(),
            preview: command.clone(),
            risk: risk_for_command(command),
            request_id: None,
            tool_use_id: None,
            tool_input: None,
            original_user_request: original_user_request.map(ToString::to_string),
            status: ApprovalRequestStatus::Pending,
            execution_path: None,
            command_block_id: None,
            redaction_status: None,
        }),
        AgentEvent::ToolPermissionRequest {
            run_id,
            request_id,
            tool_name,
            tool_input,
            tool_use_id,
            ..
        } => {
            let input_str = serde_json::to_string(tool_input).unwrap_or_default();
            let info = display_for_tool(tool_name, &input_str);
            let risk = provider_tool_permission_risk(tool_name, tool_input);
            Some(RuntimeApprovalRequest {
                id: next_approval_id(state),
                run_id: run_id.clone(),
                session_id: session_id.to_string(),
                cwd: cwd.to_string(),
                source: "control-protocol",
                provider_shell_request_kind: ProviderShellRequestKind::ControlPermission,
                kind: ApprovalRequestKind::Tool,
                subject: info.label,
                preview: info.preview,
                risk,
                request_id: Some(request_id.clone()),
                tool_use_id: Some(tool_use_id.clone()),
                tool_input: Some(tool_input.clone()),
                original_user_request: original_user_request.map(ToString::to_string),
                status: ApprovalRequestStatus::Pending,
                execution_path: Some("provider_control_protocol"),
                command_block_id: None,
                redaction_status: None,
            })
        }
        _ => None,
    }
}

fn provider_tool_permission_risk(tool_name: &str, tool_input: &serde_json::Value) -> &'static str {
    if !cosh_shell::tools::is_shell_tool_name(tool_name) {
        return "medium";
    }
    let command = tool_input
        .get("command")
        .or_else(|| tool_input.get("cmd"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    cosh_shell::tools::classify_command_interaction(command)
        .approval_risk
        .as_str()
}

fn original_user_request(run_request: Option<&AgentRequest>) -> Option<String> {
    let request = run_request?;
    request
        .user_input
        .as_ref()
        .filter(|input| !input.trim().is_empty())
        .cloned()
        .or_else(|| {
            (!request.command_block.command.trim().is_empty())
                .then(|| request.command_block.command.clone())
        })
}

pub(crate) fn record_auto_approved_request(
    state: &mut InlineState,
    mut request: RuntimeApprovalRequest,
) -> RuntimeApprovalRequest {
    request.status = ApprovalRequestStatus::Approved;
    if request.execution_path.is_none() && request.request_id.is_some() {
        request.execution_path = Some("provider_control_protocol");
    }
    state.approvals.requests.push(request.clone());
    state
        .approvals
        .journal
        .push(approval_journal_entry(&request, "agent-auto"));
    request
}

pub(crate) fn record_deferred_fallback_request(
    state: &mut InlineState,
    mut request: RuntimeApprovalRequest,
) -> RuntimeApprovalRequest {
    request.status = ApprovalRequestStatus::Blocked;
    if request.execution_path.is_none() {
        request.execution_path = Some("deferred_no_foreground_injection");
    }
    state.approvals.requests.push(request.clone());
    state
        .approvals
        .journal
        .push(approval_journal_entry(&request, "cosh-shell"));
    request
}

fn next_approval_id(state: &InlineState) -> String {
    state.approvals.next_request_id()
}

fn risk_for_command(command: &str) -> &'static str {
    cosh_shell::tools::classify_command_interaction(command)
        .approval_risk
        .as_str()
}
