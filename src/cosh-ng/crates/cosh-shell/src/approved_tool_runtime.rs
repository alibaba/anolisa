use super::*;

pub(super) fn render_approved_tool_result<W: Write>(
    state: &mut InlineState,
    request: &RuntimeApprovalRequest,
    adapter: &AdapterInstance,
    output: &mut W,
) -> std::io::Result<()> {
    if !request_is_executable_bash_tool(request) {
        return Ok(());
    }

    let run = approved_bash_tool_run(request);
    render_native_bash_tool_result(output, &run.result)?;
    record_tool_execution_result(state, request, run.result);
    start_agent_run_before_held_text(&run.continuation, adapter, state, output, None)
}

pub(super) fn request_is_executable_bash_tool(request: &RuntimeApprovalRequest) -> bool {
    request.kind == ApprovalRequestKind::Tool && is_bash_tool_subject(&request.subject)
}

fn is_bash_tool_subject(subject: &str) -> bool {
    matches!(
        subject,
        "Bash"
            | "shell"
            | "run_shell_command"
            | "tool Bash"
            | "tool shell"
            | "tool run_shell_command"
    )
}

pub(super) fn request_is_readonly_builtin_tool(request: &RuntimeApprovalRequest) -> bool {
    if request.kind != ApprovalRequestKind::Tool {
        return false;
    }

    matches!(
        request.subject.as_str(),
        "Read"
            | "Grep"
            | "Glob"
            | "LS"
            | "read_file"
            | "grep_search"
            | "glob"
            | "list_directory"
            | "read_many_files"
            | "tool Read"
            | "tool Grep"
            | "tool Glob"
            | "tool LS"
            | "tool read_file"
            | "tool grep_search"
            | "tool glob"
            | "tool list_directory"
            | "tool read_many_files"
    )
}

struct ApprovedToolRun {
    result: ToolExecutionResult,
    continuation: AgentRequest,
}

fn approved_bash_tool_run(request: &RuntimeApprovalRequest) -> ApprovedToolRun {
    let command = raw_bash_command(&request.preview);
    let result = cosh_shell::run_user_approved_bash_tool(command);
    let continuation = tool_result_agent_request(request, &result);
    ApprovedToolRun {
        result,
        continuation,
    }
}

fn render_native_bash_tool_result<W: Write>(
    output: &mut W,
    result: &ToolExecutionResult,
) -> std::io::Result<()> {
    writeln!(output, "\n$ {}", result.command)?;
    write_native_stream(output, &result.stdout)?;
    write_native_stream(output, &result.stderr)?;
    match result.status {
        ToolExecutionStatus::Executed => {
            if let Some(code) = result.exit_code.filter(|code| *code != 0) {
                writeln!(output, "exit {code}")?;
            }
        }
        ToolExecutionStatus::Blocked
        | ToolExecutionStatus::TimedOut
        | ToolExecutionStatus::Failed => {
            writeln!(output, "cosh-shell: {}", result.reason)?;
        }
    }
    Ok(())
}

fn write_native_stream<W: Write>(output: &mut W, text: &str) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    write!(output, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(output)?;
    }
    Ok(())
}

fn record_tool_execution_result(
    state: &mut InlineState,
    request: &RuntimeApprovalRequest,
    result: ToolExecutionResult,
) -> Vec<String> {
    let mut ids = Vec::new();
    if !result.stdout.trim().is_empty() {
        let id = next_activity_id(state, "out");
        let output_ref = state
            .activity_output_dir
            .as_deref()
            .and_then(|dir| write_tool_output_ref(dir, &id, &result.stdout).ok())
            .map(|path| path.display().to_string());
        state.activity_rows.push(RuntimeActivityRow {
            id: id.clone(),
            run_id: request.run_id.clone(),
            kind: ActivityKind::ToolOutput,
            status: "captured".to_string(),
            subject: request.id.clone(),
            summary: format!("stdout captured; /details {id}"),
            detail: tool_execution_output_detail(
                request,
                "stdout",
                result.stdout.lines().count(),
                output_ref.as_deref(),
                &result.stdout,
            ),
        });
        ids.push(id);
    }

    if !result.stderr.trim().is_empty() {
        let id = next_activity_id(state, "out");
        let output_ref = state
            .activity_output_dir
            .as_deref()
            .and_then(|dir| write_tool_output_ref(dir, &id, &result.stderr).ok())
            .map(|path| path.display().to_string());
        state.activity_rows.push(RuntimeActivityRow {
            id: id.clone(),
            run_id: request.run_id.clone(),
            kind: ActivityKind::ToolOutput,
            status: "captured".to_string(),
            subject: request.id.clone(),
            summary: format!("stderr captured; /details {id}"),
            detail: tool_execution_output_detail(
                request,
                "stderr",
                result.stderr.lines().count(),
                output_ref.as_deref(),
                &result.stderr,
            ),
        });
        ids.push(id);
    }

    let id = next_activity_id(state, "tool");
    state.activity_rows.push(RuntimeActivityRow {
        id: id.clone(),
        run_id: request.run_id.clone(),
        kind: ActivityKind::Tool,
        status: result.status.label().to_string(),
        subject: request.id.clone(),
        summary: tool_execution_summary(&result),
        detail: tool_execution_detail(request, &result),
    });
    ids.push(id);
    ids
}

fn tool_execution_summary(result: &ToolExecutionResult) -> String {
    match result.status {
        ToolExecutionStatus::Executed => {
            let exit = result
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            format!("exit {exit}")
        }
        ToolExecutionStatus::Blocked => "tool request blocked by shell broker guard".to_string(),
        ToolExecutionStatus::TimedOut => "tool request timed out".to_string(),
        ToolExecutionStatus::Failed => "tool request failed".to_string(),
    }
}

fn tool_execution_detail(request: &RuntimeApprovalRequest, result: &ToolExecutionResult) -> String {
    format!(
        "approval: {}\nrun: {}\nsubject: {}\ncommand: {}\nstatus: {}\nexit_code: {}\nreason: {}",
        request.id,
        request.run_id,
        request.subject,
        result.command,
        result.status.label(),
        result
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string()),
        result.reason
    )
}

fn tool_execution_output_detail(
    request: &RuntimeApprovalRequest,
    stream: &str,
    lines: usize,
    output_ref: Option<&str>,
    text: &str,
) -> String {
    let mut detail = format!(
        "approval: {}\nrun: {}\nsubject: {}\nstream: {}\nlines: {}",
        request.id, request.run_id, request.subject, stream, lines
    );
    if let Some(output_ref) = output_ref {
        detail.push_str(&format!("\nref: {output_ref}"));
    }
    detail.push('\n');
    detail.push_str(text);
    detail
}

fn raw_bash_command(preview: &str) -> &str {
    preview.strip_prefix("$ ").unwrap_or(preview)
}

fn tool_result_agent_request(
    request: &RuntimeApprovalRequest,
    result: &ToolExecutionResult,
) -> AgentRequest {
    let block_id = format!("tool-result-{}", request.id);
    let status =
        if matches!(result.status, ToolExecutionStatus::Executed) && result.exit_code == Some(0) {
            CommandStatus::Completed
        } else {
            CommandStatus::Failed
        };
    let exit_code = result.exit_code.unwrap_or(match status {
        CommandStatus::Completed => 0,
        CommandStatus::Failed => 1,
    });
    let user_input = format!(
        "Tool result for request {id}\n\
         Tool: {subject}\n\
         Command: {command}\n\
         Status: {status}\n\
         Exit code: {exit_code}\n\
         Reason: {reason}\n\
         Stdout:\n{stdout}\n\
         Stderr:\n{stderr}\n\
         Continue the same Agent session using this tool result. \
         If Status is executed, analyze only the native command output already printed above. \
         Do not repeat that approval was needed, do not list commands for the user to run manually, \
         and do not describe pre-approval recommendation steps. \
         If Status is blocked, timed_out, or failed, say the tool request did not successfully run; \
         do not diagnose it as a user shell failure, do not claim output exists, do not say it executed, \
         and issue one simpler bounded read-only shell tool call if more evidence is required. \
         Do not ask the user to run the command manually.",
        id = request.id,
        subject = request.subject,
        command = result.command,
        status = result.status.label(),
        exit_code = result
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string()),
        reason = result.reason,
        stdout = result.stdout,
        stderr = result.stderr,
    );

    AgentRequest {
        id: format!("agent-request-{block_id}"),
        session_id: request.session_id.clone(),
        command_block: CommandBlock {
            id: block_id,
            session_id: request.session_id.clone(),
            command: user_input.clone(),
            cwd: request.cwd.clone(),
            end_cwd: request.cwd.clone(),
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code,
            status,
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
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn tool_request(subject: &str) -> RuntimeApprovalRequest {
        RuntimeApprovalRequest {
            id: "req-1".to_string(),
            run_id: "run-1".to_string(),
            session_id: "session-1".to_string(),
            cwd: "/tmp".to_string(),
            source: "test",
            kind: ApprovalRequestKind::Tool,
            subject: subject.to_string(),
            preview: "$ pwd".to_string(),
            risk: "medium",
            request_id: None,
            tool_use_id: None,
            status: ApprovalRequestStatus::Pending,
        }
    }

    fn drain_agent_run(state: &mut InlineState, adapter: &AdapterInstance, output: &mut Vec<u8>) {
        for _ in 0..50 {
            poll_active_agent_run(state, output, adapter).expect("poll active agent run");
            if state.active_run.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("agent run did not finish");
    }

    #[test]
    fn executable_bash_tool_accepts_provider_aliases() {
        for subject in [
            "Bash",
            "shell",
            "run_shell_command",
            "tool Bash",
            "tool shell",
            "tool run_shell_command",
        ] {
            assert!(
                request_is_executable_bash_tool(&tool_request(subject)),
                "{subject}"
            );
        }
    }

    #[test]
    fn executable_bash_tool_rejects_non_shell_tools() {
        assert!(!request_is_executable_bash_tool(&tool_request("Read")));
    }

    #[test]
    fn readonly_builtin_tool_accepts_qwen_aliases() {
        for subject in [
            "Read",
            "Grep",
            "Glob",
            "LS",
            "read_file",
            "grep_search",
            "glob",
            "list_directory",
            "read_many_files",
        ] {
            assert!(
                request_is_readonly_builtin_tool(&tool_request(subject)),
                "{subject}"
            );
        }
    }

    #[test]
    fn approved_tool_result_continues_claude_session() {
        let adapter = AdapterInstance::ClaudeCode(cosh_shell::ClaudeCodeAdapter {
            program: "claude".to_string(),
            model: "sonnet".to_string(),
            max_budget_usd: "1".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("provider-session-claude".to_string()))),
        });
        let mut state = InlineState::default();
        let mut output = Vec::new();

        render_approved_tool_result(&mut state, &tool_request("Bash"), &adapter, &mut output)
            .expect("render approved tool result");
        drain_agent_run(&mut state, &adapter, &mut output);

        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("Prepared invocation"), "{output}");
        assert!(output.contains("--resume"), "{output}");
        assert!(output.contains("provider-session-claude"), "{output}");
        assert!(
            state
                .activity_rows
                .iter()
                .any(|row| row.summary == "exit 0"),
            "expected native Bash tool result activity row"
        );
    }

    #[test]
    fn approved_tool_result_continues_qwen_session() {
        let adapter = AdapterInstance::QwenCli(cosh_shell::QwenCliAdapter {
            program: "co".to_string(),
            allow_model_call: false,
            session_id: Arc::new(Mutex::new(Some("provider-session-qwen".to_string()))),
        });
        let mut state = InlineState::default();
        let mut output = Vec::new();

        render_approved_tool_result(&mut state, &tool_request("Bash"), &adapter, &mut output)
            .expect("render approved tool result");
        drain_agent_run(&mut state, &adapter, &mut output);

        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("Prepared invocation"), "{output}");
        assert!(output.contains("--resume"), "{output}");
        assert!(output.contains("provider-session-qwen"), "{output}");
        assert!(
            state
                .activity_rows
                .iter()
                .any(|row| row.summary == "exit 0"),
            "expected native Bash tool result activity row"
        );
    }
}
