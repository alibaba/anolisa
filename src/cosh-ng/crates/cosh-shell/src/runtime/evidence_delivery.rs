use cosh_shell::adapter::{
    ApprovalDecision, ApprovalResponse, HostExecutedShellMetadata, HostExecutedShellResult,
};
use cosh_shell::context_window::redact_provider_command_text;
use cosh_shell::types::{
    AgentMode, AgentRequest, CommandBlock, CommandStatus, OutputRefs, ShellHandoffRequest,
};

use crate::runtime::state::{InlineState, RuntimeApprovalRequest};

use super::evidence_state::{EvidenceState, RuntimeShellCommandCompleted, ShellEvidenceDelivery};

pub(crate) fn record_shell_handoff_completion(
    state: &mut InlineState,
    handoff: &ShellHandoffRequest,
    block: &CommandBlock,
    status: &'static str,
) -> RuntimeShellCommandCompleted {
    let mut evidence = RuntimeShellCommandCompleted::from_shell_handoff(handoff, block, status);
    let delivery = deliver_host_executed_shell_result_if_supported(state, handoff, &evidence);
    if delivery.delivered {
        state.agent_run.native_prompt_after_run = true;
        state.agent_run.host_executed_shell_result_delivered = true;
    }
    evidence.apply_provider_result_delivery(delivery);
    state
        .evidence
        .record_shell_command_completed(evidence.clone());
    evidence
}

pub(crate) fn shell_handoff_continuation_requests(state: &mut InlineState) -> Vec<AgentRequest> {
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
) -> Option<AgentRequest> {
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
        };
    };
    let Some(capabilities) = state
        .agent_run
        .active
        .as_ref()
        .map(|run| run.handle.control_capabilities())
    else {
        return ShellEvidenceDelivery {
            delivered: false,
            status: "provider_run_not_active",
            recovery_reason: Some(
                "provider run was not active when shell completed; shell evidence continuation required",
            ),
        };
    };
    if !capabilities.can_handle_host_executed_shell_tool_result {
        return ShellEvidenceDelivery {
            delivered: false,
            status: "unsupported",
            recovery_reason: Some(
                "provider did not advertise host-executed shell result support; shell evidence continuation required",
            ),
        };
    }

    let Some(claim) = state
        .control
        .provider_tool_mut()
        .claim_host_executed_shell_result(request_id, handoff.tool_use_id.as_deref())
    else {
        return ShellEvidenceDelivery {
            delivered: true,
            status: "duplicate_already_delivered",
            recovery_reason: None,
        };
    };

    let result = host_executed_shell_result(handoff, evidence);
    let delivered = match state.agent_run.active.as_ref() {
        Some(run) => run
            .handle
            .respond_approval(ApprovalResponse {
                request_id: request_id.clone(),
                tool_use_id: handoff.tool_use_id.clone(),
                tool_input: None,
                decision: ApprovalDecision::HostExecutedShell {
                    result: Box::new(result),
                },
            })
            .is_ok(),
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
        }
    } else {
        ShellEvidenceDelivery {
            delivered: false,
            status: "provider_channel_closed",
            recovery_reason: Some(
                "provider approval channel closed before host-executed shell result was delivered; shell evidence continuation required",
            ),
        }
    }
}

fn host_executed_shell_result(
    handoff: &ShellHandoffRequest,
    evidence: &RuntimeShellCommandCompleted,
) -> HostExecutedShellResult {
    let view = EvidenceState::provider_visible_view(evidence);
    let llm_content = format!(
        "ShellCommandCompleted evidence\n\
         {}",
        view.provider_summary,
    );
    HostExecutedShellResult {
        llm_content,
        return_display: view.return_display,
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
) -> AgentRequest {
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
    AgentRequest {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosh_shell::types::OutputRefs;

    #[test]
    fn host_executed_shell_result_uses_opaque_output_id_without_path() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-host-executed-result-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let output_ref = dir.join("cmd-1.txt");
        std::fs::write(&output_ref, "Filesystem\n/dev/disk1 10G 5G 5G\n")
            .expect("write output ref");
        let output_ref_str = output_ref.to_str().expect("utf8 output ref");

        let command = "df -h --token cli-secret";
        let mut handoff = ShellHandoffRequest::new(
            command,
            "$ df -h --token cli-secret",
            "provider-tool-call",
            "agent",
            "req-1",
            "run-1",
            10,
        )
        .expect("handoff");
        handoff.request_id = Some("ctrl-1".to_string());
        handoff.tool_use_id = Some("toolu-1".to_string());
        let block = CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "raw-session".to_string(),
            command: command.to_string(),
            cwd: "/repo".to_string(),
            end_cwd: "/repo".to_string(),
            started_at_ms: 10,
            ended_at_ms: 20,
            duration_ms: 10,
            exit_code: 0,
            status: CommandStatus::Completed,
            output: OutputRefs {
                terminal_output_ref: Some(output_ref_str.to_string()),
                terminal_output_bytes: 32,
            },
        };
        let evidence =
            RuntimeShellCommandCompleted::from_shell_handoff(&handoff, &block, "completed");

        let result = host_executed_shell_result(&handoff, &evidence);

        assert!(
            result
                .llm_content
                .contains("output_id: terminal-output://raw-session/cmd-1"),
            "{}",
            result.llm_content
        );
        assert!(
            result.llm_content.contains("bounded_output_summary:"),
            "{}",
            result.llm_content
        );
        assert!(
            result.llm_content.contains("Filesystem"),
            "{}",
            result.llm_content
        );
        assert!(
            !result.llm_content.contains(output_ref_str),
            "{}",
            result.llm_content
        );
        assert_eq!(
            result.metadata.output_ref.as_deref(),
            Some("terminal-output://raw-session/cmd-1")
        );
        assert!(
            result.metadata.command.contains("--token <redacted>"),
            "{:?}",
            result.metadata.command
        );
        assert!(
            !result.metadata.command.contains("cli-secret"),
            "{:?}",
            result.metadata.command
        );
        assert!(
            !result.llm_content.contains("cli-secret"),
            "{}",
            result.llm_content
        );
        assert_eq!(result.metadata.tool_use_id.as_deref(), Some("toolu-1"));
        assert!(
            !result
                .return_display
                .as_deref()
                .unwrap_or("")
                .contains(output_ref_str),
            "{:?}",
            result.return_display
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
