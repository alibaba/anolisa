use crate::approval::journal::approval_journal_entry;
use crate::runtime::prelude::*;

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
            let assessment = shell_tool_assessment_from_preview(&info.label, &info.preview);
            let risk = assessment
                .as_ref()
                .map(|assessment| assessment.impact.legacy_risk())
                .unwrap_or("medium");
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
                assessment: assessment.as_ref().map(runtime_assessment_summary),
            })
        }
        AgentEvent::Action { run_id, command } => {
            let assessment = shell_command_assessment(command);
            Some(RuntimeApprovalRequest {
                id: next_approval_id(state),
                run_id: run_id.clone(),
                session_id: session_id.to_string(),
                cwd: cwd.to_string(),
                source: "agent",
                provider_shell_request_kind: ProviderShellRequestKind::LocalApproval,
                kind: ApprovalRequestKind::ShellCommand,
                subject: "shell command".to_string(),
                preview: command.clone(),
                risk: assessment.impact.legacy_risk(),
                request_id: None,
                tool_use_id: None,
                tool_input: None,
                original_user_request: original_user_request.map(ToString::to_string),
                status: ApprovalRequestStatus::Pending,
                execution_path: None,
                command_block_id: None,
                redaction_status: None,
                assessment: Some(runtime_assessment_summary(&assessment)),
            })
        }
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
            let assessment = provider_tool_permission_assessment(tool_name, tool_input);
            let risk = assessment
                .as_ref()
                .map(|assessment| assessment.impact.legacy_risk())
                .unwrap_or("medium");
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
                assessment: assessment.as_ref().map(runtime_assessment_summary),
            })
        }
        _ => None,
    }
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

pub(crate) fn refresh_shell_request_assessment(
    request: &mut RuntimeApprovalRequest,
    policy: AssessmentPolicy,
) -> Option<CommandAssessment> {
    if !is_shell_tool_name(&request.subject) {
        return None;
    }
    let command = request
        .preview
        .strip_prefix("$ ")
        .unwrap_or(&request.preview)
        .trim();
    let assessment = if command.is_empty() {
        blocked_shell_binding_assessment(policy.source, command, "unsafe-binding")
    } else {
        assess_shell_command(command, policy)
    };
    request.risk = assessment.impact.legacy_risk();
    request.assessment = Some(runtime_assessment_summary(&assessment));
    Some(assessment)
}

fn next_approval_id(state: &InlineState) -> String {
    state.approvals.next_request_id()
}

fn shell_tool_assessment_from_preview(subject: &str, preview: &str) -> Option<CommandAssessment> {
    if !is_shell_tool_name(subject) {
        return None;
    }
    let command = preview.strip_prefix("$ ").unwrap_or(preview).trim();
    if command.is_empty() {
        return Some(blocked_shell_binding_assessment(
            AssessmentSource::ProviderShellTool,
            command,
            "unsafe-binding",
        ));
    }
    Some(shell_command_assessment(command))
}

fn provider_tool_permission_assessment(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> Option<CommandAssessment> {
    if !is_shell_tool_name(tool_name) {
        return None;
    }
    let command = tool_input
        .get("command")
        .or_else(|| tool_input.get("cmd"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    if command.is_empty() {
        return Some(blocked_shell_binding_assessment(
            AssessmentSource::ProviderShellTool,
            command,
            "unsafe-binding",
        ));
    }
    Some(shell_command_assessment(command))
}

fn shell_command_assessment(command: &str) -> CommandAssessment {
    assess_shell_command(
        command,
        AssessmentPolicy::ask(AssessmentSource::ProviderShellTool),
    )
}

fn runtime_assessment_summary(assessment: &CommandAssessment) -> RuntimeCommandAssessmentSummary {
    RuntimeCommandAssessmentSummary {
        impact: assessment.impact.legacy_risk(),
        execution: execution_label(assessment.execution),
        confidence: confidence_label(assessment.confidence),
        primary_reason: assessment.primary_reason(),
        reason_trace: assessment.reason_trace(),
        auto_allow: assessment.auto_allow.map(AutoAllowEvidence::reason_code),
        output_stability: output_stability_label(assessment.output_stability),
        output_exposure: output_exposure_label(assessment.output_exposure),
    }
}

fn execution_label(decision: ExecutionDecision) -> &'static str {
    match decision {
        ExecutionDecision::AutoAllow => "auto-allow",
        ExecutionDecision::AskUser => "ask-user",
        ExecutionDecision::Block => "block",
        ExecutionDecision::ForegroundHandoffRequired => "foreground-handoff-required",
    }
}

fn confidence_label(confidence: AssessmentConfidence) -> &'static str {
    match confidence {
        AssessmentConfidence::High => "high",
        AssessmentConfidence::Medium => "medium",
        AssessmentConfidence::Low => "low",
    }
}

fn output_stability_label(stability: CommandRiskOutputStability) -> &'static str {
    match stability {
        CommandRiskOutputStability::StableSnapshot => "stable-snapshot",
        CommandRiskOutputStability::PotentiallyLarge => "potentially-large",
        CommandRiskOutputStability::Streaming => "streaming",
        CommandRiskOutputStability::UnstableInteractive => "unstable-interactive",
    }
}

fn output_exposure_label(exposure: OutputExposure) -> &'static str {
    match exposure {
        OutputExposure::Normal => "normal",
        OutputExposure::MayContainCommandLine => "may-contain-command-line",
        OutputExposure::MayContainEnvironment => "may-contain-environment",
        OutputExposure::MayContainSecrets => "may-contain-secrets",
    }
}
