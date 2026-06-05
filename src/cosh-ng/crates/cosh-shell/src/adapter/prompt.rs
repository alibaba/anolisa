use crate::types::AgentRequest;

pub fn prompt_from_request(request: &AgentRequest) -> String {
    let context = recent_context_prompt(request);
    let hook_hints = command_hook_hints_prompt(request);
    if let Some(input) = &request.user_input {
        if input.starts_with("Answer to pending Agent question:") {
            return format!(
                "Continue the same Shell-first Agent session using this user answer.\n\
                 Do not ask the same question again. Do not treat this answer as a shell command. \
                 No shell command ran while collecting the answer.\n\
                 Use the answer to continue the prior task, and keep the response concise.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 question_answer:\n{}\n\
                 cwd: {}\n\
                 mode: {:?}{}{}",
                input, request.command_block.cwd, request.mode, context, hook_hints
            );
        }

        if input.starts_with("Tool result for approved request ") {
            return format!(
                "Continue the same Shell-first Agent session using this approved tool result.\n\
                 The native shell transcript has already printed the command and stdout/stderr. \
                 Any earlier pre-approval prose in this same session is obsolete. \
                 Analyze only the result below. Do not repeat that approval was needed, do not list \
                 commands for the user to run manually, do not describe pre-approval steps, and \
                 do not continue an earlier recommendation list.\n\
                 If the status is blocked, timed_out, or failed, say the command did not \
                 successfully run and request one simpler read-only Bash tool command only if \
                 more evidence is required.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 tool_result:\n{}\n\
                 cwd: {}\n\
                 mode: {:?}{}{}",
                input, request.command_block.cwd, request.mode, context, hook_hints
            );
        }

        if input.starts_with("Approval result for request ") {
            return format!(
                "Continue the same Shell-first Agent session using this approval decision.\n\
                 No shell command ran for this request. Do not claim the command executed and \
                 do not invent output. Provide a safe next step or ask for another approval only \
                 if more evidence is required.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 approval_result:\n{}\n\
                 cwd: {}\n\
                 mode: {:?}{}{}",
                input, request.command_block.cwd, request.mode, context, hook_hints
            );
        }

        return format!(
            "Handle this natural-language shell prompt request for a Shell-first assistant.\n\
             Return explanation and recommended next commands only. Do not execute commands.\n\
             If the user explicitly asks to run or execute a shell command, request the Bash tool \
             for that exact command instead of describing it in prose; cosh-shell will review the \
             request and will not execute it automatically.\n\
             For Bash tool requests, use one read-only command at a time; avoid pipes, redirects, \
             command chains, command substitution, and quotes.\n\
             If more user input is needed, request AskUserQuestion with the visible question text \
             and 2-4 concrete options; allow free text for an Other answer when appropriate.\n\
             Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
             user_input: {}\n\
             cwd: {}\n\
             mode: {:?}{}{}",
            input, request.command_block.cwd, request.mode, context, hook_hints
        );
    }

    let findings = request
        .findings
        .iter()
        .map(|finding| format!("- {:?}: {}", finding.kind, finding.message))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Analyze this failed shell command for a Shell-first assistant.\n\
         Return explanation and recommended next commands only. Do not execute commands.\n\
         Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
         command: {}\n\
         cwd: {}\n\
         exit_code: {}\n\
         terminal_output_ref: {}\n\
         findings:\n{}{}{}",
        request.command_block.command,
        request.command_block.cwd,
        request.command_block.exit_code,
        request
            .command_block
            .output
            .terminal_output_ref
            .as_deref()
            .unwrap_or("<missing>"),
        findings,
        context,
        hook_hints
    )
}

fn recent_context_prompt(request: &AgentRequest) -> String {
    if request.context_blocks.is_empty() {
        return String::new();
    }

    let lines = request
        .context_blocks
        .iter()
        .map(|block| {
            format!(
                "- {} cwd={} exit={} output_ref={} command={}",
                block.id,
                block.cwd,
                block.exit_code,
                block
                    .output
                    .terminal_output_ref
                    .as_deref()
                    .unwrap_or("<missing>"),
                block.command
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "\n\nRecent shell context:\n{}\nUse output_ref only when needed; do not claim command output you have not read.",
        lines
    )
}

fn command_hook_hints_prompt(request: &AgentRequest) -> String {
    if request.context_hints.is_empty() {
        return String::new();
    }

    let lines = request
        .context_hints
        .iter()
        .map(|hint| format!("- {hint}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "\n\nCommand result hook hints:\n{}\nTreat these as routing hints only; inspect referenced output_ref before claiming details.",
        lines
    )
}

#[cfg(test)]
mod tests {
    use super::prompt_from_request;
    use crate::types::{AgentMode, AgentRequest, CommandBlock, CommandStatus, OutputRefs};

    #[test]
    fn prompt_includes_recent_shell_context_refs_without_full_output() {
        let mut request = AgentRequest {
            id: "agent-request-input-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("input-1", "please explain context", 0, None),
            context_blocks: vec![command_block(
                "cmd-1",
                "echo shell-context-ok",
                0,
                Some("/tmp/cosh-out/cmd-1.txt"),
            )],
            context_hints: Vec::new(),
            user_input: Some("please explain context".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
        };

        let prompt = prompt_from_request(&request);
        assert!(prompt.contains("Recent shell context:"), "{prompt}");
        assert!(prompt.contains("cmd-1 cwd=/repo exit=0"), "{prompt}");
        assert!(
            prompt.contains("output_ref=/tmp/cosh-out/cmd-1.txt"),
            "{prompt}"
        );
        assert!(prompt.contains("command=echo shell-context-ok"), "{prompt}");
        assert!(
            prompt.contains("Use output_ref only when needed"),
            "{prompt}"
        );

        request.context_blocks.clear();
        let prompt_without_context = prompt_from_request(&request);
        assert!(
            !prompt_without_context.contains("Recent shell context:"),
            "{prompt_without_context}"
        );
    }

    #[test]
    fn prompt_includes_command_result_hook_hints() {
        let request = AgentRequest {
            id: "agent-request-input-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("input-1", "please explain context", 0, None),
            context_blocks: Vec::new(),
            context_hints: vec![
                "hook-hint-cmd-1 block=cmd-1 command failed; output_ref=/tmp/cosh-out/cmd-1.txt"
                    .to_string(),
            ],
            user_input: Some("please explain context".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
        };

        let prompt = prompt_from_request(&request);
        assert!(prompt.contains("Command result hook hints:"), "{prompt}");
        assert!(
            prompt.contains("output_ref=/tmp/cosh-out/cmd-1.txt"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Treat these as routing hints only"),
            "{prompt}"
        );
    }

    fn command_block(
        id: &str,
        command: &str,
        exit_code: i32,
        output_ref: Option<&str>,
    ) -> CommandBlock {
        CommandBlock {
            id: id.to_string(),
            session_id: "session-1".to_string(),
            command: command.to_string(),
            cwd: "/repo".to_string(),
            end_cwd: "/repo".to_string(),
            started_at_ms: 1,
            ended_at_ms: 2,
            duration_ms: 1,
            exit_code,
            status: if exit_code == 0 {
                CommandStatus::Completed
            } else {
                CommandStatus::Failed
            },
            output: OutputRefs {
                terminal_output_ref: output_ref.map(ToString::to_string),
                terminal_output_bytes: 24,
            },
        }
    }
}
