use crate::context_window::{build_context_window, format_context_prompt, ContextWindowConfig};
use crate::types::{AgentRequest, CoshApprovalMode};

pub fn prompt_from_request(request: &AgentRequest) -> String {
    let context = rich_context_prompt(request);
    let hook_hints = command_hook_hints_prompt(request);
    let base = if let Some(input) = &request.user_input {
        if input.starts_with("Answer to pending Agent question:") {
            format!(
                "Continue the same Shell-first Agent session using this user answer.\n\
                 Do not ask the same question again. Do not treat this answer as a shell command. \
                 No shell command ran while collecting the answer.\n\
                 Use the answer to continue the prior task, and keep the response concise.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 question_answer:\n{}\n\
                 cwd: {}\n\
                 mode: {:?}{}{}",
                input, request.command_block.cwd, request.mode, context, hook_hints
            )
        } else if input.starts_with("Tool result for request ")
            || input.starts_with("Tool result for approved request ")
        {
            format!(
                "Continue the same Shell-first Agent session using this tool result.\n\
                 The native shell transcript has already printed the command and stdout/stderr. \
                 Any earlier pre-approval prose in this same session is obsolete. \
                 Analyze only the result below. Do not repeat that approval was needed, do not list \
                 commands for the user to run manually, do not describe pre-approval steps, and \
                 do not continue an earlier recommendation list.\n\
                 If the status is blocked, timed_out, or failed, say the command did not \
                 successfully run, do not diagnose it as a user shell failure, and issue one \
                 simpler bounded read-only shell tool call only if more evidence is required.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 tool_result:\n{}\n\
                 cwd: {}\n\
                 mode: {:?}{}{}",
                input, request.command_block.cwd, request.mode, context, hook_hints
            )
        } else if input.starts_with("Approval result for request ") {
            format!(
                "Continue the same Shell-first Agent session using this approval decision.\n\
                 No shell command ran for this request. Do not claim the command executed and \
                 do not invent output. Provide a safe next step or ask for another approval only \
                 if more evidence is required.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 approval_result:\n{}\n\
                 cwd: {}\n\
                 mode: {:?}{}{}",
                input, request.command_block.cwd, request.mode, context, hook_hints
            )
        } else {
            format!(
                "Handle this natural-language shell prompt request for a Shell-first assistant.\n\
                 Decide based on user intent:\n\
                 - If the user wants to DO something (view files, check status, run tests, inspect system, debug), \
                 use the Bash tool directly. cosh-shell has an approval system that reviews every tool request \
                 before execution.\n\
                 - If the user wants to KNOW something (ask a question, request explanation, compare options), \
                 answer in prose with example commands in code blocks.\n\
                 Prefer one bounded read-only Bash command at a time when that is enough. \
                 If shell syntax such as pipes, redirects, or command chains materially improves the task, \
                 use it as a Bash tool request and let cosh-shell ask for confirmation when required.\n\
                 If more user input is needed, request AskUserQuestion with the visible question text \
                 and 2-4 concrete options; allow free text for an Other answer when appropriate.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 user_input: {}\n\
                 cwd: {}\n\
                 mode: {:?}{}{}",
                input, request.command_block.cwd, request.mode, context, hook_hints
            )
        }
    } else {
        let findings = request
            .findings
            .iter()
            .map(|finding| format!("- {:?}: {}", finding.kind, finding.message))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Analyze this failed shell command for a Shell-first assistant.\n\
             First use the Read tool to inspect the terminal_output_ref if available. \
             Then explain the failure and suggest fixes. \
             cosh-shell has an approval system that reviews every tool request.\n\
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
    };

    let hook_suffix = hook_finding_prompt(request);
    if hook_suffix.is_empty() {
        base
    } else {
        format!("{base}{hook_suffix}")
    }
}

pub fn provider_prompt_contract(mode: CoshApprovalMode, shell_tool_name: &str) -> String {
    let target_mode = match mode {
        CoshApprovalMode::Suggest => "recommend",
        CoshApprovalMode::Ask | CoshApprovalMode::Auto | CoshApprovalMode::Trust => "agent",
    };
    let mode_instruction = if target_mode == "recommend" {
        "This invocation is recommend mode: do not emit tool calls. Answer with concise guidance, explanations, and example commands in code blocks."
    } else {
        "This invocation is agent mode: when the user asks to inspect system, project, file, test, runtime, or command state, actively use tools for live evidence instead of only suggesting commands for the user to run."
    };

    format!(
        "\n\ncosh-shell Agent contract:\n\
         - Target user modes are recommend and agent.\n\
         - recommend means recommend/explain only; agent means tool-capable execution under cosh-shell governance.\n\
         - {mode_instruction}\n\
         - Use the provider shell tool `{shell_tool_name}` for live shell evidence when tool use is needed.\n\
         - Simple bounded read-only shell tool calls may be auto-approved by cosh-shell. \
         Shell syntax such as pipes, redirects, command chains, quotes, globs, and command substitution is supported after user approval. \
         Prefer simple commands when they are enough, but do not avoid useful shell syntax by asking the user to run commands manually.\n\
         - The approval system is handled by cosh-shell; do not downgrade to a manual command suggestion only because approval may be needed.\n\
         - Keep provider-specific names out of the visible response unless they are command/tool labels already shown by cosh-shell."
    )
}

fn hook_finding_prompt(request: &AgentRequest) -> String {
    let Some(finding) = &request.hook_finding else {
        return String::new();
    };
    let skill = request
        .recommended_skill
        .as_deref()
        .or(finding.skill.as_deref())
        .unwrap_or("none");
    format!(
        "\n\nHook finding: {}\nDescription: {}\nRecommended skill: {}",
        finding.title, finding.description, skill
    )
}

fn rich_context_prompt(request: &AgentRequest) -> String {
    if request.context_blocks.is_empty() {
        return String::new();
    }

    let before_ms = request
        .context_blocks
        .iter()
        .map(|b| b.ended_at_ms)
        .max()
        .unwrap_or(0)
        + 1;
    let config = ContextWindowConfig::default();
    let entries = build_context_window(&request.context_blocks, before_ms, &config);
    format_context_prompt(&entries)
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
    use super::{prompt_from_request, provider_prompt_contract};
    use crate::hook_types::HookFinding;
    use crate::types::{
        AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, OutputRefs,
    };

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
            hook_finding: None,
            recommended_skill: None,
        };

        let prompt = prompt_from_request(&request);
        assert!(
            prompt.contains("Recent shell context (1 commands)"),
            "{prompt}"
        );
        assert!(prompt.contains("[cmd-1]"), "{prompt}");
        assert!(prompt.contains("exit=0"), "{prompt}");
        assert!(prompt.contains("cwd=/repo"), "{prompt}");
        assert!(prompt.contains("ref=/tmp/cosh-out/cmd-1.txt"), "{prompt}");
        assert!(prompt.contains("echo shell-context-ok"), "{prompt}");
        assert!(
            prompt.contains("Use Read tool on output_ref paths"),
            "{prompt}"
        );

        request.context_blocks.clear();
        let prompt_without_context = prompt_from_request(&request);
        assert!(
            !prompt_without_context.contains("Recent shell context"),
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
            hook_finding: None,
            recommended_skill: None,
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

    #[test]
    fn prompt_appends_hook_finding_when_present() {
        let request = AgentRequest {
            id: "agent-request-input-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("input-1", "please explain context", 0, None),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("please explain context".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: Some(HookFinding {
                hook_id: "test-failure".to_string(),
                severity: crate::hook_types::FindingSeverity::Warning,
                title: "Test failed".to_string(),
                description: "cargo test exited with code 101".to_string(),
                suggestion: "Use /rust-project".to_string(),
                skill: Some("rust-project".to_string()),
                cli_hint: None,
                context_refs: Vec::new(),
            }),
            recommended_skill: None,
        };

        let prompt = prompt_from_request(&request);
        assert!(prompt.contains("Hook finding: Test failed"), "{prompt}");
        assert!(
            prompt.contains("Description: cargo test exited with code 101"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Recommended skill: rust-project"),
            "{prompt}"
        );
    }

    #[test]
    fn prompt_omits_hook_finding_when_none() {
        let request = AgentRequest {
            id: "agent-request-input-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("input-1", "please explain context", 0, None),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("please explain context".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        };

        let prompt = prompt_from_request(&request);
        assert!(!prompt.contains("Hook finding:"), "{prompt}");
    }

    #[test]
    fn blocked_tool_result_prompt_keeps_provider_on_tool_path() {
        let input = "Tool result for request req-1\n\
            Tool: run_shell_command\n\
            Command: brew install git\n\
            Status: timed_out\n\
            Exit code: none\n\
            Reason: user-approved Bash tool timed out\n\
            Stdout:\n\
            Stderr:\n";
        let request = AgentRequest {
            id: "agent-request-tool-result-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("tool-result-1", input, 1, None),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some(input.to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        };

        let prompt = prompt_from_request(&request);
        assert!(
            prompt.contains("issue one simpler bounded read-only shell tool call"),
            "{prompt}"
        );
        assert!(
            prompt.contains("do not list commands for the user to run manually"),
            "{prompt}"
        );
        assert!(
            prompt.contains("do not diagnose it as a user shell failure"),
            "{prompt}"
        );
    }

    #[test]
    fn provider_prompt_contract_describes_recommend_mode_without_tools() {
        let prompt = provider_prompt_contract(CoshApprovalMode::Suggest, "run_shell_command");

        assert!(prompt.contains("recommend"), "{prompt}");
        assert!(prompt.contains("agent"), "{prompt}");
        assert!(prompt.contains("do not emit tool calls"), "{prompt}");
        assert!(prompt.contains("run_shell_command"), "{prompt}");
    }

    #[test]
    fn provider_prompt_contract_describes_agent_mode_with_cosh_approval() {
        let prompt = provider_prompt_contract(CoshApprovalMode::Auto, "run_shell_command");

        assert!(prompt.contains("agent mode"), "{prompt}");
        assert!(prompt.contains("actively use tools"), "{prompt}");
        assert!(
            prompt.contains("approval system is handled by cosh-shell"),
            "{prompt}"
        );
        assert!(prompt.contains("run_shell_command"), "{prompt}");
    }

    #[test]
    fn provider_prompt_contract_describes_shell_syntax_approval_boundary() {
        let prompt = provider_prompt_contract(CoshApprovalMode::Auto, "run_shell_command");

        assert!(prompt.contains("auto-approved"), "{prompt}");
        assert!(prompt.contains("Shell syntax"), "{prompt}");
        assert!(prompt.contains("after user approval"), "{prompt}");
        assert!(
            prompt.contains("do not avoid useful shell syntax"),
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
