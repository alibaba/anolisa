use crate::agent::continuation::run_request_is_analysis_only_continuation;
use crate::agent::run::stop_active_agent_run_without_rendering;
use crate::approval::broker::{
    approval_execution_metadata, classify_approval_outcome, provider_allow_response,
    provider_deny_response, ApprovalExecutionMetadata, ApprovalOutcome, ApprovalOutcomeInput,
    ProviderApprovalStatus, ProviderResponseInput,
};
use crate::approval::resolution::request_can_receive_host_executed_result;
use crate::runtime::prelude::*;

pub(crate) fn render_trusted_tool<W: Write>(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    run_request: Option<&AgentRequest>,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<bool> {
    if state.approval_mode != CoshApprovalMode::Trust {
        return Ok(false);
    }

    for event in governed_events {
        let provider_tool_call_fallback = adapter.capabilities().control_protocol
            && matches!(event.event, AgentEvent::ToolCall { .. });
        let Some(mut request) = approval_request_from_governed_event(
            state,
            event,
            run_request,
            adapter.capabilities().control_protocol && !provider_tool_call_fallback,
        ) else {
            continue;
        };
        if provider_tool_call_fallback && !request_is_executable_bash_tool(&request) {
            continue;
        }
        if provider_tool_call_fallback {
            request.source = "provider-tool-call";
        }
        if !provider_tool_call_fallback && defer_fallback_bash_tool(state, request.clone(), output)?
        {
            return Ok(true);
        }
        if request_is_executable_bash_tool(&request)
            && (shell_tool_must_be_denied_after_foreground_evidence(state, run_request)
                || state
                    .control
                    .provider_shell_handoff_run_seen(&request.run_id)
                || run_already_approved_shell_tool(state, &request.run_id))
        {
            deny_shell_tool_during_analysis_continuation(state, &request);
            return Ok(true);
        }
        let mut request = record_auto_approved_request(state, request);
        if apply_auto_approved_request_outcome(
            state,
            &mut request,
            cosh_shell::MessageId::ApprovalResolutionAutoApprovedTitle,
            output,
        )? == AutoApprovalFlow::Handled
        {
            return Ok(true);
        }
    }

    Ok(false)
}

pub(crate) fn render_auto_approved_tool<W: Write>(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
    run_request: Option<&AgentRequest>,
    output: &mut W,
    adapter: &AdapterInstance,
) -> std::io::Result<bool> {
    if state.approval_mode != CoshApprovalMode::Auto {
        return Ok(false);
    }

    for event in governed_events {
        let provider_tool_call_fallback = adapter.capabilities().control_protocol
            && matches!(event.event, AgentEvent::ToolCall { .. });
        let Some(mut request) = approval_request_from_governed_event(
            state,
            event,
            run_request,
            adapter.capabilities().control_protocol && !provider_tool_call_fallback,
        ) else {
            continue;
        };
        if provider_tool_call_fallback && !request_is_executable_bash_tool(&request) {
            continue;
        }
        if provider_tool_call_fallback {
            request.source = "provider-tool-call";
        }
        if request_is_readonly_builtin_tool(&request) {
            let mut request = record_auto_approved_request(state, request);
            if apply_auto_approved_request_outcome(
                state,
                &mut request,
                cosh_shell::MessageId::ApprovalResolutionAutoApprovedTitle,
                output,
            )? == AutoApprovalFlow::Handled
            {
                return Ok(true);
            }
            continue;
        }
        if request_is_executable_bash_tool(&request)
            && (shell_tool_must_be_denied_after_foreground_evidence(state, run_request)
                || state
                    .control
                    .provider_shell_handoff_run_seen(&request.run_id)
                || run_already_approved_shell_tool(state, &request.run_id))
        {
            deny_shell_tool_during_analysis_continuation(state, &request);
            return Ok(true);
        }

        let raw_cmd = request
            .preview
            .strip_prefix("$ ")
            .unwrap_or(&request.preview);

        if request_is_executable_bash_tool(&request)
            && command_matches_trust_key(raw_cmd, state.control.session_trusted_commands())
        {
            if defer_fallback_bash_tool(state, request.clone(), output)? {
                return Ok(true);
            }
            let mut request = record_auto_approved_request(state, request);
            if apply_auto_approved_request_outcome(
                state,
                &mut request,
                cosh_shell::MessageId::ApprovalResolutionTrustedTitle,
                output,
            )? == AutoApprovalFlow::Handled
            {
                return Ok(true);
            }
            continue;
        }

        if !request_is_executable_bash_tool(&request)
            || can_run_approved_bash_tool(raw_cmd).is_err()
        {
            continue;
        }

        if request_is_executable_bash_tool(&request)
            && request.request_id.is_none()
            && !provider_tool_call_fallback
        {
            if defer_fallback_bash_tool(state, request, output)? {
                return Ok(true);
            }
            continue;
        }

        let mut request = record_auto_approved_request(state, request);
        if apply_auto_approved_request_outcome(
            state,
            &mut request,
            cosh_shell::MessageId::ApprovalResolutionAutoApprovedTitle,
            output,
        )? == AutoApprovalFlow::Handled
        {
            return Ok(true);
        }
    }

    Ok(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoApprovalFlow {
    Continue,
    Handled,
}

fn approval_outcome_for_auto_request(request: &RuntimeApprovalRequest) -> ApprovalOutcome {
    classify_approval_outcome(ApprovalOutcomeInput {
        approved: request.status == ApprovalRequestStatus::Approved,
        shell_tool: request_is_executable_bash_tool(request),
        provider_request: request.provider_shell_request_kind.is_control_permission(),
    })
}

fn shell_tool_must_be_denied_after_foreground_evidence(
    state: &InlineState,
    run_request: Option<&AgentRequest>,
) -> bool {
    run_request_is_analysis_only_continuation(run_request)
        || state.evidence.has_open_provider_shell_evidence()
        || state.agent_run.host_executed_shell_result_delivered
}

fn run_already_approved_shell_tool(state: &InlineState, run_id: &str) -> bool {
    state.approvals.requests.iter().any(|request| {
        request.run_id == run_id
            && request.status == ApprovalRequestStatus::Approved
            && request_is_executable_bash_tool(request)
    })
}

fn apply_auto_approved_request_outcome<W: Write>(
    state: &mut InlineState,
    request: &mut RuntimeApprovalRequest,
    title: cosh_shell::MessageId,
    output: &mut W,
) -> std::io::Result<AutoApprovalFlow> {
    let outcome = approval_outcome_for_auto_request(request);
    if outcome == ApprovalOutcome::ProviderNativeShellFallback {
        mark_provider_native_shell_execution(state, request);
    }
    render_approval_resolution(state, request, title, output)?;

    match outcome {
        ApprovalOutcome::ProviderNativeShellFallback => {
            respond_provider_native_shell_fallback(state, request);
            Ok(AutoApprovalFlow::Continue)
        }
        ApprovalOutcome::ProviderApprovalResponse => {
            respond_auto_approval_to_provider(state, request);
            Ok(AutoApprovalFlow::Continue)
        }
        ApprovalOutcome::LocalOnly => Ok(AutoApprovalFlow::Continue),
        ApprovalOutcome::ForegroundShellHandoff => {
            queue_approved_shell_handoff(state, request);
            if !request_can_receive_host_executed_result(state, request) {
                stop_active_agent_run_without_rendering(state, output)?;
            }
            Ok(AutoApprovalFlow::Handled)
        }
    }
}

fn respond_provider_native_shell_fallback(
    state: &InlineState,
    request: &RuntimeApprovalRequest,
) -> bool {
    let Some(request_id) = request.request_id.as_ref() else {
        return false;
    };
    let Some(active_run) = state.agent_run.active.as_ref() else {
        return true;
    };
    active_run
        .handle
        .respond_approval(provider_allow_response(ProviderResponseInput {
            request_id,
            tool_use_id: request.tool_use_id.as_deref(),
            tool_input: request.tool_input.as_ref(),
        }))
        .is_ok()
}

fn mark_provider_native_shell_execution(
    state: &mut InlineState,
    request: &mut RuntimeApprovalRequest,
) {
    let metadata = approval_execution_metadata(
        ApprovalOutcome::ProviderNativeShellFallback,
        ProviderApprovalStatus::Approved,
        request_is_executable_bash_tool(request),
    );
    set_approval_execution_metadata(state, &request.id, metadata);
    apply_approval_execution_metadata(request, metadata);
}

fn set_approval_execution_metadata(
    state: &mut InlineState,
    approval_id: &str,
    metadata: ApprovalExecutionMetadata,
) {
    for request in &mut state.approvals.requests {
        if request.id == approval_id {
            apply_approval_execution_metadata(request, metadata);
        }
    }
    for entry in &mut state.approvals.journal {
        if entry.id == approval_id {
            entry.execution_path = metadata.execution_path;
            entry.redaction_status = metadata.redaction_status;
        }
    }
}

fn apply_approval_execution_metadata(
    request: &mut RuntimeApprovalRequest,
    metadata: ApprovalExecutionMetadata,
) {
    request.execution_path = metadata.execution_path;
    request.redaction_status = metadata.redaction_status;
}

fn defer_fallback_bash_tool<W: Write>(
    state: &mut InlineState,
    request: RuntimeApprovalRequest,
    output: &mut W,
) -> std::io::Result<bool> {
    if !request_is_executable_bash_tool(&request)
        || request.provider_shell_request_kind.is_control_permission()
    {
        return Ok(false);
    }
    let request = record_deferred_fallback_request(state, request);
    render_approval_resolution(
        state,
        &request,
        cosh_shell::MessageId::ApprovalResolutionDeferredTitle,
        output,
    )?;
    stop_active_agent_run_without_rendering(state, output)?;
    Ok(true)
}

fn respond_auto_approval_to_provider(
    state: &InlineState,
    request: &RuntimeApprovalRequest,
) -> bool {
    if request_is_executable_bash_tool(request) {
        return false;
    }
    let Some(request_id) = request.request_id.as_ref() else {
        return false;
    };
    let Some(active_run) = state.agent_run.active.as_ref() else {
        return true;
    };
    let response = match request.tool_use_id.as_ref() {
        Some(tool_use_id) => provider_allow_response(ProviderResponseInput {
            request_id,
            tool_use_id: Some(tool_use_id),
            tool_input: request.tool_input.as_ref(),
        }),
        None => provider_deny_response(
            ProviderResponseInput {
                request_id,
                tool_use_id: None,
                tool_input: request.tool_input.as_ref(),
            },
            "Missing provider tool_use_id".to_string(),
        ),
    };
    let _ = active_run.handle.respond_approval(response);
    true
}

fn deny_shell_tool_during_analysis_continuation(
    state: &InlineState,
    request: &RuntimeApprovalRequest,
) -> bool {
    let Some(request_id) = request.request_id.as_ref() else {
        return false;
    };
    let Some(active_run) = state.agent_run.active.as_ref() else {
        return true;
    };
    let response = provider_deny_response(
        ProviderResponseInput {
            request_id,
            tool_use_id: request.tool_use_id.as_deref(),
            tool_input: request.tool_input.as_ref(),
        },
        "The foreground shell command already completed and its output was injected. Summarize the existing shell evidence or ask the user to start a new request before running another shell command.".to_string(),
    );
    let _ = active_run.handle.respond_approval(response);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis_only_request() -> AgentRequest {
        AgentRequest {
            id: "agent-request-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: CommandBlock {
                id: "cmd-1".to_string(),
                session_id: "session-1".to_string(),
                command: "ShellCommandCompleted evidence".to_string(),
                cwd: "/tmp".to_string(),
                end_cwd: "/tmp".to_string(),
                started_at_ms: 0,
                ended_at_ms: 0,
                duration_ms: 0,
                exit_code: 0,
                status: CommandStatus::Completed,
                output: OutputRefs {
                    terminal_output_ref: None,
                    terminal_output_bytes: 0,
                },
            },
            context_blocks: Vec::new(),
            context_hints: vec![
                "analysis-only continuation after foreground shell handoff".to_string()
            ],
            user_input: Some("ShellCommandCompleted evidence".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }

    #[test]
    fn analysis_only_continuation_blocks_streamed_shell_tool_fallback() {
        let adapter = AdapterInstance::QwenCli(cosh_shell::adapter::QwenCliAdapter::default());
        let mut state = InlineState {
            approval_mode: CoshApprovalMode::Auto,
            ..InlineState::default()
        };
        let governed = GovernedEvent {
            decision: cosh_shell::types::GovernanceDecision::Display,
            policy_decision: cosh_shell::types::GovernancePolicyDecision::NeedsUserApproval,
            event: AgentEvent::ToolCall {
                run_id: "run-1".to_string(),
                tool_id: None,
                name: "run_shell_command".to_string(),
                input: r#"{"command":"df -h"}"#.to_string(),
            },
            reason: "visible streamed tool call".to_string(),
            display_text: "visible streamed tool call".to_string(),
            auto_execute: false,
        };
        let mut output = Vec::new();

        let handled = render_auto_approved_tool(
            &mut state,
            &[governed],
            Some(&analysis_only_request()),
            &mut output,
            &adapter,
        )
        .expect("render auto approval");

        assert!(handled);
        assert!(state.approvals.requests.is_empty());
        assert!(state.control.shell_handoff().approved_is_empty());
    }
}
