use crate::runtime::prelude::{
    redact_provider_command_text, AgentContextBinding, AgentMode, AgentRequest, AgentRunOrigin,
    ApprovalDecision, ApprovalResponse, CommandBlock, CommandStatus, HostExecutedShellMetadata,
    HostExecutedShellResult, OutputRefs, ShellHandoffRequest,
};
use crate::runtime::state::{InlineState, RuntimeApprovalRequest};

use super::evidence_state::{EvidenceState, RuntimeShellCommandCompleted, ShellEvidenceDelivery};

pub(crate) fn record_shell_handoff_completion(
    state: &mut InlineState,
    handoff: &ShellHandoffRequest,
    block: &CommandBlock,
    status: &'static str,
) -> RuntimeShellCommandCompleted {
    let origin = state
        .approvals
        .requests
        .iter()
        .find(|request| request.id == handoff.approval_id)
        .map(|request| request.origin)
        .or_else(|| {
            state
                .control
                .find_interactive_shell_handoff(&handoff.approval_id)
                .map(|handoff| handoff.origin)
        })
        .unwrap_or_default();
    let mut evidence =
        RuntimeShellCommandCompleted::from_shell_handoff(handoff, block, status, origin);
    let delivery = deliver_host_executed_shell_result_if_supported(state, handoff, &evidence);
    if delivery.delivered {
        state.agent_run.native_prompt_after_run = true;
        state.agent_run.host_executed_shell_result_delivered = true;
        if evidence.terminal_output_ref.is_some() {
            let output_id = crate::evidence::output_policy::terminal_output_id(
                &evidence.shell_session_id,
                &evidence.command_block_id,
            );
            let run_id = state
                .agent_run
                .active
                .as_ref()
                .map(|run| run.request.id.clone());
            let summary_complete = delivery.provider_preview_complete;
            state.shell_evidence.record_host_executed_shell_output(
                output_id,
                run_id,
                summary_complete,
            );
        }
    }
    evidence.apply_provider_result_delivery(delivery);
    state
        .evidence
        .record_shell_command_completed(evidence.clone());
    evidence
}

pub(crate) fn shell_handoff_continuation_requests(
    state: &mut InlineState,
) -> Vec<(AgentRequest, AgentRunOrigin)> {
    let mut requests = Vec::new();
    for evidence in state.evidence.claim_pending_shell_handoff_continuations() {
        let Some(approval_id) = evidence.approval_id.as_ref() else {
            continue;
        };
        let approval = state
            .approvals
            .requests
            .iter()
            .find(|request| request.id == *approval_id);
        requests.push(shell_handoff_continuation_request(&evidence, approval));
    }
    requests
}

pub(crate) fn stalled_provider_shell_handoff_continuation_request(
    state: &mut InlineState,
) -> Option<(AgentRequest, AgentRunOrigin)> {
    let evidence = state
        .evidence
        .claim_stalled_provider_shell_handoff_continuations()
        .into_iter()
        .next()?;
    let approval_id = evidence.approval_id.as_ref()?;
    let approval = state
        .approvals
        .requests
        .iter()
        .find(|request| request.id == *approval_id);
    Some(shell_handoff_continuation_request(&evidence, approval))
}

fn deliver_host_executed_shell_result_if_supported(
    state: &mut InlineState,
    handoff: &ShellHandoffRequest,
    evidence: &RuntimeShellCommandCompleted,
) -> ShellEvidenceDelivery {
    let Some(request_id) = handoff.request_id.as_ref() else {
        return ShellEvidenceDelivery {
            delivered: false,
            status: "not_provider_tool_request",
            recovery_reason: Some("no provider request id; shell evidence continuation required"),
            provider_preview_complete: false,
        };
    };
    let Some(active_run) = state.agent_run.active.as_ref() else {
        return ShellEvidenceDelivery {
            delivered: false,
            status: "provider_run_not_active",
            recovery_reason: Some(
                "provider run was not active when shell completed; shell evidence continuation required",
            ),
            provider_preview_complete: false,
        };
    };
    if active_run.request.id != handoff.run_id {
        return ShellEvidenceDelivery {
            delivered: false,
            status: "provider_run_not_owner",
            recovery_reason: Some(
                "provider run no longer owns the shell handoff; shell evidence continuation required",
            ),
            provider_preview_complete: false,
        };
    }
    let capabilities = active_run.handle.control_capabilities();
    if !capabilities.can_handle_host_executed_shell_tool_result {
        return ShellEvidenceDelivery {
            delivered: false,
            status: "unsupported",
            recovery_reason: Some(
                "provider did not advertise host-executed shell result support; shell evidence continuation required",
            ),
            provider_preview_complete: false,
        };
    }

    let Some(claim) = state
        .control
        .provider_tool_mut()
        .claim_host_executed_shell_result(
            &handoff.run_id,
            request_id,
            handoff.tool_use_id.as_deref(),
        )
    else {
        return ShellEvidenceDelivery {
            delivered: true,
            status: "duplicate_already_delivered",
            recovery_reason: None,
            provider_preview_complete: false,
        };
    };

    let view = EvidenceState::provider_visible_view(evidence);
    let provider_preview_complete = view.provider_preview_complete;
    let result = host_executed_shell_result_from_view(handoff, evidence, view);
    let delivered = match state.agent_run.active.as_mut() {
        Some(run) => {
            let delivered = run
                .handle
                .respond_approval(ApprovalResponse {
                    request_id: request_id.clone(),
                    tool_use_id: handoff.tool_use_id.clone(),
                    tool_input: None,
                    decision: ApprovalDecision::HostExecutedShell {
                        result: Box::new(result),
                    },
                })
                .is_ok();
            if delivered {
                run.last_activity_at = std::time::Instant::now();
            }
            delivered
        }
        None => false,
    };
    if !delivered {
        state
            .control
            .provider_tool_mut()
            .release_host_executed_shell_result(claim);
    }
    if delivered {
        ShellEvidenceDelivery {
            delivered: true,
            status: "delivered",
            recovery_reason: None,
            provider_preview_complete,
        }
    } else {
        ShellEvidenceDelivery {
            delivered: false,
            status: "provider_channel_closed",
            recovery_reason: Some(
                "provider approval channel closed before host-executed shell result was delivered; shell evidence continuation required",
            ),
            provider_preview_complete,
        }
    }
}

#[cfg(test)]
fn host_executed_shell_result(
    handoff: &ShellHandoffRequest,
    evidence: &RuntimeShellCommandCompleted,
) -> HostExecutedShellResult {
    let view = EvidenceState::provider_visible_view(evidence);
    host_executed_shell_result_from_view(handoff, evidence, view)
}

fn host_executed_shell_result_from_view(
    handoff: &ShellHandoffRequest,
    evidence: &RuntimeShellCommandCompleted,
    view: crate::evidence::output_policy::EvidenceView,
) -> HostExecutedShellResult {
    let llm_content = format!(
        "ShellCommandCompleted evidence\n\
         {}",
        view.provider_summary,
    );
    HostExecutedShellResult {
        llm_content,
        return_display: None,
        metadata: HostExecutedShellMetadata {
            command: redact_provider_command_text(&evidence.command),
            status: evidence.status.to_string(),
            exit_code: evidence.exit_code,
            signal: None,
            cwd: evidence.cwd.clone(),
            end_cwd: evidence.end_cwd.clone(),
            duration_ms: evidence.duration_ms,
            output_ref: evidence.terminal_output_ref.as_ref().map(|_| {
                crate::evidence::output_policy::terminal_output_id(
                    &evidence.shell_session_id,
                    &evidence.command_block_id,
                )
            }),
            redaction_status: view.redaction_status.to_string(),
            approval_id: evidence.approval_id.clone(),
            tool_use_id: handoff.tool_use_id.clone(),
        },
    }
}

fn shell_handoff_continuation_request(
    evidence: &RuntimeShellCommandCompleted,
    approval: Option<&RuntimeApprovalRequest>,
) -> (AgentRequest, AgentRunOrigin) {
    let approval_id = evidence.approval_id.as_deref().unwrap_or("<none>");
    let subject = approval
        .map(|request| request.subject.as_str())
        .unwrap_or("<unknown>");
    let provider_request_id = approval
        .and_then(|request| request.request_id.as_deref())
        .unwrap_or("<none>");
    let tool_use_id = approval
        .and_then(|request| request.tool_use_id.as_deref())
        .unwrap_or("<none>");
    let original_user_request = approval
        .and_then(|request| request.original_user_request.as_deref())
        .unwrap_or("<unknown>");
    let view = EvidenceState::provider_visible_view(evidence);
    let user_input = format!(
        "ShellCommandCompleted evidence\n\
         The foreground shell executed this command after user approval. Treat this as shell evidence, not as a provider-native tool_result.\n\
         Continue the analysis-only Agent turn from the prior request. Further shell commands require a fresh approval.\n\
         original_user_request: {original_user_request}\n\
         approval_id: {approval_id}\n\
         provider_tool: {subject}\n\
         provider_request_id: {provider_request_id}\n\
         tool_use_id: {tool_use_id}\n\
         {}",
        view.provider_summary,
    );
    let mut request = AgentRequest {
        id: format!("agent-request-shell-evidence-{approval_id}"),
        session_id: approval
            .map(|request| request.session_id.clone())
            .unwrap_or_else(|| "shell-handoff-session".to_string()),
        command_block: CommandBlock {
            id: format!("shell-evidence-{approval_id}"),
            session_id: approval
                .map(|request| request.session_id.clone())
                .unwrap_or_else(|| "shell-handoff-session".to_string()),
            command: user_input.clone(),
            origin: Default::default(),
            cwd: evidence.end_cwd.clone(),
            end_cwd: evidence.end_cwd.clone(),
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code: evidence.exit_code,
            status: if evidence.exit_code == 0 {
                CommandStatus::Completed
            } else {
                CommandStatus::Failed
            },
            output: OutputRefs {
                terminal_output_ref: evidence.terminal_output_ref.clone(),
                terminal_output_bytes: 0,
            },
            shell_environment_generation: None,
        },
        context_blocks: Vec::new(),
        context_hints: vec![
            "analysis-only continuation after foreground shell handoff".to_string(),
            format!(
                "shell handoff recovery owner: {approval_id}/{provider_request_id}/{tool_use_id}"
            ),
            "do not reuse the prior approval for a new shell command".to_string(),
        ],
        user_input: Some(user_input),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    };
    crate::types::set_request_context_binding(
        &mut request,
        AgentContextBinding::ShellHandoffContinuation,
    );
    (request, evidence.origin)
}

#[cfg(test)]
#[path = "evidence_delivery_tests.rs"]
mod tests;
