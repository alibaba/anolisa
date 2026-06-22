use crate::runtime::prelude::{
    redact_provider_command_text, AgentMode, AgentRequest, ApprovalDecision, ApprovalResponse,
    CommandBlock, CommandStatus, HostExecutedShellMetadata, HostExecutedShellResult, OutputRefs,
    ShellHandoffRequest,
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
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::runtime::prelude::{
        AgentEvent, AgentRunHandle, AgentRunPoll, CoshApprovalMode, CoshCoreAdapter, Language,
        OutputRefs, RatatuiInlineRenderer,
    };

    use crate::agent::run::ActiveAgentRun;

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
            origin: Default::default(),
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

    #[test]
    fn host_executed_delivery_channel_closed_records_recovery_and_releases_claim() {
        let request = test_request();
        let handle = closed_cosh_core_control_handle(&request);
        assert!(
            handle
                .control_capabilities()
                .can_handle_host_executed_shell_tool_result,
            "mock provider must advertise host-executed support before exiting"
        );

        let mut state = InlineState::default();
        state.agent_run.active = Some(test_active_run(request, handle));
        let mut handoff = ShellHandoffRequest::new(
            "df -h",
            "$ df -h",
            "provider-tool-call",
            "agent",
            "req-1",
            "run-1",
            10,
        )
        .expect("handoff");
        handoff.request_id = Some("ctrl-closed".to_string());
        handoff.tool_use_id = Some("toolu-closed".to_string());
        let block = CommandBlock {
            id: "cmd-closed".to_string(),
            session_id: "raw-session".to_string(),
            command: "df -h".to_string(),
            origin: Default::default(),
            cwd: "/repo".to_string(),
            end_cwd: "/repo".to_string(),
            started_at_ms: 10,
            ended_at_ms: 20,
            duration_ms: 10,
            exit_code: 0,
            status: CommandStatus::Completed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 32,
            },
        };
        let evidence =
            RuntimeShellCommandCompleted::from_shell_handoff(&handoff, &block, "completed");

        let delivery =
            deliver_host_executed_shell_result_if_supported(&mut state, &handoff, &evidence);

        assert!(!delivery.delivered);
        assert_eq!(delivery.status, "provider_channel_closed");
        assert!(
            delivery
                .recovery_reason
                .unwrap_or_default()
                .contains("approval channel closed"),
            "{delivery:?}"
        );
        assert!(
            state
                .control
                .provider_tool_mut()
                .claim_host_executed_shell_result("ctrl-closed", Some("toolu-closed"))
                .is_some(),
            "failed delivery must release duplicate guard claim"
        );
    }

    #[test]
    fn host_executed_delivery_refreshes_active_run_idle_clock() {
        let request = test_request();
        let (dir, handle) = open_cosh_core_control_handle(&request);

        let mut state = InlineState::default();
        state.agent_run.active = Some(test_active_run(request, handle));
        state
            .agent_run
            .active
            .as_mut()
            .expect("active run")
            .last_activity_at = Instant::now() - Duration::from_secs(60);
        let mut handoff = ShellHandoffRequest::new(
            "df -h",
            "$ df -h",
            "provider-tool-call",
            "agent",
            "req-1",
            "run-1",
            10,
        )
        .expect("handoff");
        handoff.request_id = Some("ctrl-open".to_string());
        handoff.tool_use_id = Some("toolu-open".to_string());
        let block = CommandBlock {
            id: "cmd-open".to_string(),
            session_id: "raw-session".to_string(),
            command: "df -h".to_string(),
            origin: Default::default(),
            cwd: "/repo".to_string(),
            end_cwd: "/repo".to_string(),
            started_at_ms: 10,
            ended_at_ms: 20,
            duration_ms: 20_000,
            exit_code: 0,
            status: CommandStatus::Completed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 32,
            },
        };
        let evidence =
            RuntimeShellCommandCompleted::from_shell_handoff(&handoff, &block, "completed");

        let delivery =
            deliver_host_executed_shell_result_if_supported(&mut state, &handoff, &evidence);

        let refreshed = state
            .agent_run
            .active
            .as_ref()
            .expect("active run")
            .last_activity_at;
        assert!(delivery.delivered, "{delivery:?}");
        assert!(
            refreshed.elapsed() < Duration::from_secs(2),
            "host-executed delivery should reset provider idle clock; elapsed={:?}",
            refreshed.elapsed()
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    fn open_cosh_core_control_handle(request: &AgentRequest) -> (PathBuf, AgentRunHandle) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-open-control-{}-{unique}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let program = dir.join("cosh-core-open-control.sh");
        std::fs::write(
            &program,
            r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","model":"mock-cosh-core","session_id":"mock-open-control"}'
read -r user_message
printf '%s\n' '{"type":"control_request","request_id":"ctrl-open","request":{"subtype":"can_use_tool","tool_name":"run_shell_command","input":{"command":"df -h"},"tool_use_id":"toolu-open"}}'
if IFS= read -r response; then
  case "$response" in
    *'"behavior":"host_executed_shell"'*'df -h'*)
      printf '%s\n' '{"type":"assistant","session_id":"mock-open-control","message":{"content":[{"type":"text","text":"host executed accepted"}]}}'
      printf '%s\n' '{"type":"result","subtype":"success","session_id":"mock-open-control","is_error":false,"result":"done"}'
      exit 0
      ;;
  esac
fi
printf '%s\n' '{"type":"result","subtype":"error","session_id":"mock-open-control","is_error":true,"result":"missing host executed response"}'
exit 1
"#,
        )
        .expect("write mock cosh-core");
        let mut permissions = std::fs::metadata(&program)
            .expect("mock metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&program, permissions).expect("chmod mock cosh-core");
        let adapter = CoshCoreAdapter {
            program: program.to_string_lossy().to_string(),
            allow_model_call: true,
            session_id: Arc::default(),
            session_cwd: Arc::default(),
        };
        let handle = adapter.start_cancellable(request.clone(), CoshApprovalMode::Auto);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_request = false;
        while Instant::now() < deadline {
            match handle.poll_event_timeout(Duration::from_millis(100)) {
                Ok(AgentRunPoll::Event(AgentEvent::ToolPermissionRequest { .. })) => {
                    saw_request = true;
                    break;
                }
                Ok(AgentRunPoll::Event(_)) | Ok(AgentRunPoll::Timeout) => continue,
                Ok(AgentRunPoll::Finished) => break,
                Err(err) => panic!("mock cosh-core control run failed: {err:?}"),
            }
        }
        assert!(saw_request, "mock provider did not emit tool permission");
        assert!(
            handle
                .control_capabilities()
                .can_handle_host_executed_shell_tool_result,
            "mock provider must advertise host-executed support"
        );
        (dir, handle)
    }

    fn closed_cosh_core_control_handle(request: &AgentRequest) -> AgentRunHandle {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let adapter = CoshCoreAdapter {
            program: manifest_dir
                .join("tests")
                .join("fixtures")
                .join("provider")
                .join("qwen")
                .join("mock_qwen_control_capabilities.sh")
                .to_string_lossy()
                .to_string(),
            allow_model_call: true,
            session_id: Arc::default(),
            session_cwd: Arc::default(),
        };
        let handle = adapter.start_cancellable(request.clone(), CoshApprovalMode::Auto);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match handle.poll_event_timeout(Duration::from_millis(100)) {
                Ok(AgentRunPoll::Event(AgentEvent::AgentCompleted { .. })) => break,
                Ok(AgentRunPoll::Event(_)) | Ok(AgentRunPoll::Timeout) => continue,
                Ok(AgentRunPoll::Finished) => break,
                Err(err) => panic!("mock cosh-core control run failed: {err:?}"),
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        handle
    }

    fn test_active_run(request: AgentRequest, handle: AgentRunHandle) -> ActiveAgentRun {
        let renderer = RatatuiInlineRenderer::for_terminal();
        ActiveAgentRun {
            request,
            handle,
            provider_name: "cosh-core",
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

    fn test_request() -> AgentRequest {
        AgentRequest {
            id: "request-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: CommandBlock {
                id: "cmd-request".to_string(),
                session_id: "session-1".to_string(),
                command: "df -h".to_string(),
                origin: Default::default(),
                cwd: "/repo".to_string(),
                end_cwd: "/repo".to_string(),
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
            user_input: Some("check disk".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        }
    }
}
