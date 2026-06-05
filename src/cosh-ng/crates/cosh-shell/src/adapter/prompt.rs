use crate::context_window::{
    build_context_window, format_context_prompt, provider_safe_command_facts, ContextWindowConfig,
};
use crate::types::{AgentRequest, CoshApprovalMode};

pub fn prompt_from_request(request: &AgentRequest) -> String {
    let mut prompt = trigger_evidence_prompt(request);
    prompt.push_str(&runtime_frame_prompt(request));
    prompt.push_str(&hook_finding_prompt(request));
    prompt
}

fn trigger_evidence_prompt(request: &AgentRequest) -> String {
    if let Some(input) = &request.user_input {
        if input.starts_with("Answer to pending Agent question:") {
            format!(
                "Continue the same Shell-first Agent session using this user answer.\n\
                 Do not ask the same question again. Do not treat this answer as a shell command. \
                 No shell command ran while collecting the answer.\n\
                 Use the answer to continue the prior task, and keep the response concise.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 question_answer:\n{}\n\
                 ",
                input
            )
        } else if input.starts_with("Tool result for request ")
            || input.starts_with("Tool result for approved request ")
        {
            format!(
                "Continue the same Shell-first Agent session using this tool result.\n\
                 The native shell transcript has already printed the command and stdout/stderr. \
                 The tool_result payload is a bounded model view: use preview/ref fields, do not \
                 assume it contains the full output. \
                 Any earlier pre-approval prose in this same session is obsolete. \
                 Analyze only the result below. Do not repeat that approval was needed, do not list \
                 commands for the user to run manually, do not describe pre-approval steps, and \
                 do not continue an earlier recommendation list.\n\
                 If the status is blocked, timed_out, or failed, say the command did not \
                 successfully run, do not diagnose it as a user shell failure, and issue one \
                 simpler bounded read-only shell tool call only if more evidence is required.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 tool_result:\n{}\n\
                 ",
                input
            )
        } else if input.starts_with("Approval result for request ") {
            format!(
                "Continue the same Shell-first Agent session using this approval decision.\n\
                 No shell command ran for this request. Do not claim the command executed and \
                 do not invent output. Provide a safe next step or ask for another approval only \
                 if more evidence is required.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 approval_result:\n{}\n\
                 ",
                input
            )
        } else if input.starts_with("ShellEvidenceExcerpt\n") {
            format!(
                "Continue the same Shell-first Agent session using this user-requested shell evidence excerpt.\n\
                 The excerpt is bounded and may not contain the full command output. \
                 terminal-output:// refs are cosh-shell evidence ids, not files; do not use provider file tools to read them. \
                 If more shell evidence is needed, ask through the cosh-shell evidence request protocol instead of guessing. \
                 Do not execute follow-up commands automatically unless the user asks for further live inspection.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 shell_evidence_excerpt:\n{}\n\
                 ",
                input
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
                 history_access: Recent shell history is not included by default. If prior commands are needed, emit exactly one fenced cosh-request block: ```cosh-request\nhistory\n```.\n\
                 Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
                 user_input: {}\n\
                 ",
                input
            )
        }
    } else {
        let findings = request
            .findings
            .iter()
            .map(|finding| format!("- {:?}: {}", finding.kind, finding.message))
            .collect::<Vec<_>>()
            .join("\n");

        let command_facts = provider_safe_command_facts(&request.command_block);
        format!(
            "Analyze this failed shell command for a Shell-first assistant.\n\
             Use the included bounded shell context and output id; terminal-output:// refs are \
             not files and must not be read with provider file tools. If more output is required, \
             ask through the cosh-shell evidence request protocol. Then explain the failure and suggest fixes. \
             cosh-shell has an approval system that reviews every tool request.\n\
             Do not mention Claude Code, plan mode, implementation status, or internal workflow.\n\n\
             command: {}\n\
             exit_code: {}\n\
             output_id: {}\n\
             findings:\n{}",
            command_facts.command,
            request.command_block.exit_code,
            command_facts.output_id,
            findings
        )
    }
}

pub fn provider_prompt_contract(mode: CoshApprovalMode, shell_tool_name: &str) -> String {
    provider_prompt_contract_for_language(
        mode,
        shell_tool_name,
        crate::language_config_status().effective,
    )
}

pub fn provider_prompt_contract_for_language(
    mode: CoshApprovalMode,
    shell_tool_name: &str,
    language: crate::Language,
) -> String {
    let target_mode = match mode {
        CoshApprovalMode::Recommend => "recommend",
        CoshApprovalMode::Auto | CoshApprovalMode::Trust => "agent",
    };
    let mode_instruction = if target_mode == "recommend" {
        "This invocation is recommend mode: do not emit tool calls. Answer with concise guidance, explanations, and example commands in code blocks."
    } else {
        "This invocation is agent mode: when the user asks to inspect system, project, file, test, runtime, or command state, actively use tools for live evidence instead of only suggesting commands for the user to run."
    };

    let language_hint = provider_language_hint(language);

    invariant_contract_prompt(
        target_mode,
        mode_instruction,
        shell_tool_name,
        language_hint,
    )
}

fn invariant_contract_prompt(
    target_mode: &str,
    mode_instruction: &str,
    shell_tool_name: &str,
    language_hint: &str,
) -> String {
    format!(
        "\n\ncosh-shell Agent contract:\n\
         - User modes: recommend and agent.\n\
         - Mode: {target_mode}. {mode_instruction}\n\
         - Use `{shell_tool_name}` for live shell evidence when tool use is needed.\n\
         - Always emit a provider permission request for `{shell_tool_name}` before any shell command executes, even read-only commands in auto approval mode. \
         cosh-shell may auto-approve safe commands, but it still needs the request so the exact command can run in the foreground shell transcript. \
         Shell syntax is supported after cosh-shell approval; do not avoid useful shell syntax by asking the user to run commands manually.\n\
         - terminal-output:// refs are cosh-shell evidence ids, not files. Do not use provider file tools to read them. For more captured output, emit exactly one fenced cosh-request block: ```cosh-request\noutput <output_id> tail\nlines <n>\n```.\n\
         - The approval system is handled by cosh-shell; do not downgrade to manual command suggestions only because approval may be needed.\n\
         - {language_hint}\n\
         - Keep provider-specific names out of visible responses unless already shown by cosh-shell."
    )
}

pub fn provider_language_hint(language: crate::Language) -> &'static str {
    match language {
        crate::Language::EnUs => "Respond in English unless the user explicitly asks otherwise.",
        crate::Language::ZhCn => {
            "Respond in Simplified Chinese unless the user explicitly asks otherwise."
        }
    }
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

fn runtime_frame_prompt(request: &AgentRequest) -> String {
    format!(
        "\n\nruntime_frame:\n\
         cwd: {}\n\
         mode: {:?}{}{}",
        request.command_block.cwd,
        request.mode,
        rich_context_prompt(request),
        hook_routing_hints_prompt(request)
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
    let config = ContextWindowConfig {
        preview_enabled: false,
        max_commands: request.context_blocks.len(),
        ..Default::default()
    };
    let entries = build_context_window(&request.context_blocks, before_ms, &config);
    format_context_prompt(&entries)
}

fn hook_routing_hints_prompt(request: &AgentRequest) -> String {
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
        "\n\nHook routing hints:\n{}\nTreat these as routing hints only; use included bounded evidence or request more through cosh-shell evidence requests.",
        lines
    )
}

#[cfg(test)]
mod tests {
    use super::{
        prompt_from_request, provider_prompt_contract, provider_prompt_contract_for_language,
    };
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
        assert!(prompt.contains("runtime_frame:"), "{prompt}");
        assert!(prompt.contains("cwd: /repo"), "{prompt}");
        assert!(prompt.contains("mode: RecommendOnly"), "{prompt}");
        assert!(
            prompt.contains("Recent shell context (1 commands)"),
            "{prompt}"
        );
        assert!(prompt.contains("[cmd-1]"), "{prompt}");
        assert!(prompt.contains("exit=0"), "{prompt}");
        assert!(prompt.contains("cwd=/repo"), "{prompt}");
        assert!(
            prompt.contains("id=terminal-output://session-1/cmd-1"),
            "{prompt}"
        );
        assert!(!prompt.contains("ref=/tmp/cosh-out/cmd-1.txt"), "{prompt}");
        assert!(prompt.contains("echo shell-context-ok"), "{prompt}");
        assert!(
            prompt.contains("terminal-output:// refs are cosh-shell evidence ids"),
            "{prompt}"
        );

        request.context_blocks.clear();
        let prompt_without_context = prompt_from_request(&request);
        assert!(
            !prompt_without_context.contains("Recent shell context"),
            "{prompt_without_context}"
        );
        assert!(
            prompt_without_context
                .contains("history_access: Recent shell history is not included by default"),
            "{prompt_without_context}"
        );
        assert!(
            prompt_without_context.contains("```cosh-request\nhistory\n```"),
            "{prompt_without_context}"
        );
    }

    #[test]
    fn prompt_context_blocks_do_not_include_history_output_preview() {
        let dir = std::env::temp_dir().join(format!("cosh-prompt-context-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let output_ref = dir.join("cmd-1.txt");
        std::fs::write(&output_ref, "secret-history-output\n").expect("write output");
        let request = AgentRequest {
            id: "agent-request-input-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("input-1", "please explain context", 0, None),
            context_blocks: vec![command_block(
                "cmd-1",
                "cat history.log",
                0,
                Some(output_ref.to_str().expect("utf8 output path")),
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
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            prompt.contains("Recent shell context (1 commands)"),
            "{prompt}"
        );
        assert!(prompt.contains("cat history.log"), "{prompt}");
        assert!(
            prompt.contains("id=terminal-output://session-1/cmd-1"),
            "{prompt}"
        );
        assert!(!prompt.contains("secret-history-output"), "{prompt}");
        assert!(!prompt.contains("preview:"), "{prompt}");
    }

    #[test]
    fn prompt_includes_hook_routing_hints() {
        let request = AgentRequest {
            id: "agent-request-input-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("input-1", "please explain context", 0, None),
            context_blocks: Vec::new(),
            context_hints: vec![
                "hook-hint-cmd-1 block=cmd-1 command failed; output_id=terminal-output://session-1/cmd-1"
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
        assert!(prompt.contains("Hook routing hints:"), "{prompt}");
        assert!(
            prompt.contains("output_id=terminal-output://session-1/cmd-1"),
            "{prompt}"
        );
        assert!(!prompt.contains("/tmp/cosh-out/cmd-1.txt"), "{prompt}");
        assert!(
            !prompt.contains("inspect referenced output_ref"),
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
            Run: run-1\n\
            Command: brew install git\n\
            Status: timed_out\n\
            Exit code: none\n\
            Reason: user-approved Bash tool timed out\n\
            Stdout preview:\n\
            Stdout ref: <none>\n\
            Stderr preview:\n\
            Stderr ref: <none>\n\
            Terminal output ref: <none>\n\
            Full output was shown to the user transcript; inspect refs only if needed.\n";
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
    fn shell_evidence_excerpt_prompt_uses_explicit_follow_up_boundary() {
        let input = "ShellEvidenceExcerpt\n\
            output_id: terminal-output://session-1/cmd-1\n\
            command_id: cmd-1\n\
            command: df -h\n\
            excerpt_status: included\n\
            redaction_status: excerpt_included\n\
            bounded_output_excerpt:\n\
            Filesystem  Size  Used\n";
        let request = AgentRequest {
            id: "details-evidence-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("cmd-1", "df -h", 0, Some("/tmp/internal-output.txt")),
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
            prompt.contains("user-requested shell evidence excerpt"),
            "{prompt}"
        );
        assert!(prompt.contains("shell_evidence_excerpt:"), "{prompt}");
        assert!(
            prompt.contains("terminal-output:// refs are cosh-shell evidence ids, not files"),
            "{prompt}"
        );
        assert!(
            !prompt.contains("Handle this natural-language shell prompt request"),
            "{prompt}"
        );
        assert!(!prompt.contains("/tmp/internal-output.txt"), "{prompt}");
    }

    #[test]
    fn prompt_context_budget_tiers_are_trigger_scoped() {
        let dir = std::env::temp_dir().join(format!("cosh-prompt-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let output_ref = dir.join("cmd-ctx.txt");
        std::fs::write(&output_ref, "secret-history-output\n").expect("write output");
        let context_block = command_block(
            "cmd-ctx",
            "cat history.log",
            0,
            Some(output_ref.to_str().expect("utf8 output path")),
        );

        let free_form = prompt_from_request(&AgentRequest {
            id: "free-form".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("input-1", "analyze this", 0, None),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("analyze this".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        });
        assert!(free_form.contains("history_access:"), "{free_form}");
        assert!(!free_form.contains("Recent shell context"), "{free_form}");
        assert!(
            !free_form.contains("terminal-output://session-1/cmd-ctx"),
            "{free_form}"
        );

        let failed = prompt_from_request(&AgentRequest {
            id: "failed".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block(
                "cmd-failed",
                "curl --token cli-secret https://example.test/?password=query-secret",
                2,
                None,
            ),
            context_blocks: vec![context_block.clone()],
            context_hints: Vec::new(),
            user_input: None,
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        });
        assert!(
            failed.contains("Analyze this failed shell command"),
            "{failed}"
        );
        assert!(
            failed.contains("Recent shell context (1 commands)"),
            "{failed}"
        );
        assert!(
            failed.contains("id=terminal-output://session-1/cmd-ctx"),
            "{failed}"
        );
        assert!(failed.contains("--token <redacted>"), "{failed}");
        assert!(failed.contains("password=<redacted>"), "{failed}");
        assert!(!failed.contains("cli-secret"), "{failed}");
        assert!(!failed.contains("query-secret"), "{failed}");
        assert!(!failed.contains("secret-history-output"), "{failed}");
        assert!(!failed.contains(output_ref.to_str().unwrap()), "{failed}");

        let hook = prompt_from_request(&AgentRequest {
            id: "hook".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("cmd-hook", "free -m", 0, None),
            context_blocks: vec![context_block],
            context_hints: vec![
                "hook-hint-cmd-ctx block=cmd-ctx output_id=terminal-output://session-1/cmd-ctx"
                    .to_string(),
            ],
            user_input: Some("analyze hook finding".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: Some(HookFinding {
                hook_id: "memory-pressure".to_string(),
                severity: crate::hook_types::FindingSeverity::Warning,
                title: "Memory pressure".to_string(),
                description: "available memory is low".to_string(),
                suggestion: "Inspect memory consumers".to_string(),
                skill: None,
                cli_hint: None,
                context_refs: Vec::new(),
            }),
            recommended_skill: None,
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert!(hook.contains("Hook routing hints:"), "{hook}");
        assert!(hook.contains("Hook finding: Memory pressure"), "{hook}");
        assert!(
            hook.contains("id=terminal-output://session-1/cmd-ctx"),
            "{hook}"
        );
        assert!(!hook.contains("secret-history-output"), "{hook}");

        let host_executed = prompt_from_request(&AgentRequest {
            id: "host-result".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("cmd-host", "df -h", 0, None),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some(
                "Tool result for request req-1\n\
                 Command: df -h\n\
                 Status: executed\n\
                 bounded_output_summary:\n\
                 Filesystem Size Used\n"
                    .to_string(),
            ),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        });
        assert!(host_executed.contains("tool_result:"), "{host_executed}");
        assert!(
            host_executed.contains("bounded model view: use preview/ref fields"),
            "{host_executed}"
        );
        assert!(
            !host_executed.contains("Recent shell context"),
            "{host_executed}"
        );

        let context_follow_up = prompt_from_request(&AgentRequest {
            id: "history-follow-up".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("cmd-history", "history", 0, None),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some(
                "ShellEvidenceExcerpt\n\
                 history_limit: 20\n\
                 history_index:\n\
                 [cmd-1] exit=0 cwd=/repo output_id=terminal-output://session-1/cmd-1\n"
                    .to_string(),
            ),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        });
        assert!(
            context_follow_up.contains("shell_evidence_excerpt:"),
            "{context_follow_up}"
        );
        assert!(
            context_follow_up.contains("history_index:"),
            "{context_follow_up}"
        );
        assert!(
            !context_follow_up.contains("bounded_output_excerpt:"),
            "{context_follow_up}"
        );

        let evidence_follow_up = prompt_from_request(&AgentRequest {
            id: "output-follow-up".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("cmd-output", "df -h", 0, None),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some(
                "ShellEvidenceExcerpt\n\
                 output_id: terminal-output://session-1/cmd-output\n\
                 command_id: cmd-output\n\
                 bounded_output_excerpt:\n\
                 Filesystem Size Used\n"
                    .to_string(),
            ),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        });
        assert!(
            evidence_follow_up.contains("shell_evidence_excerpt:"),
            "{evidence_follow_up}"
        );
        assert!(
            evidence_follow_up.contains("bounded_output_excerpt:"),
            "{evidence_follow_up}"
        );
        assert!(
            !evidence_follow_up.contains("Handle this natural-language shell prompt request"),
            "{evidence_follow_up}"
        );
    }

    #[test]
    fn tool_result_prompt_declares_preview_ref_boundary() {
        let input = "Tool result for request req-1\n\
            Tool: Bash\n\
            Run: run-1\n\
            Command: sleep 1; echo a; sleep 1; echo b\n\
            Status: executed\n\
            Exit code: 0\n\
            Reason: user-approved Bash tool executed through bash -lc\n\
            Stdout preview:\n\
            a\n\
            b\n\
            Stdout ref: /tmp/cosh/out-1.txt\n\
            Stderr preview:\n\
            Stderr ref: <none>\n\
            Terminal output ref: /tmp/cosh/out-1.txt\n\
            Full output was shown to the user transcript; inspect refs only if needed.\n";
        let request = AgentRequest {
            id: "agent-request-tool-result-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("tool-result-1", input, 0, Some("/tmp/cosh/out-1.txt")),
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
            prompt.contains("bounded model view: use preview/ref fields"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Stdout ref: /tmp/cosh/out-1.txt"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Full output was shown to the user transcript"),
            "{prompt}"
        );
    }

    #[test]
    fn provider_prompt_contract_describes_recommend_mode_without_tools() {
        let prompt = provider_prompt_contract(CoshApprovalMode::Recommend, "run_shell_command");

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

        assert!(prompt.contains("auto-approve"), "{prompt}");
        assert!(
            prompt.contains("Always emit a provider permission request"),
            "{prompt}"
        );
        assert!(prompt.contains("foreground shell transcript"), "{prompt}");
        assert!(prompt.contains("Shell syntax"), "{prompt}");
        assert!(prompt.contains("after cosh-shell approval"), "{prompt}");
        assert!(
            prompt.contains("do not avoid useful shell syntax"),
            "{prompt}"
        );
    }

    #[test]
    fn provider_prompt_contract_describes_evidence_request_boundary() {
        let prompt = provider_prompt_contract(CoshApprovalMode::Auto, "run_shell_command");

        assert!(
            prompt.contains("terminal-output:// refs are cosh-shell evidence ids, not files"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Do not use provider file tools to read them"),
            "{prompt}"
        );
        assert!(prompt.contains("```cosh-request"), "{prompt}");
        assert!(prompt.contains("output <output_id> tail"), "{prompt}");
        assert!(prompt.contains("lines <n>"), "{prompt}");
        assert!(
            !prompt.contains("Read tool on output_ref paths"),
            "{prompt}"
        );
    }

    #[test]
    fn provider_prompt_contract_includes_language_hint_without_losing_governance() {
        let en = provider_prompt_contract_for_language(
            CoshApprovalMode::Recommend,
            "run_shell_command",
            crate::Language::EnUs,
        );
        let zh = provider_prompt_contract_for_language(
            CoshApprovalMode::Auto,
            "run_shell_command",
            crate::Language::ZhCn,
        );

        assert!(en.contains("Respond in English"), "{en}");
        assert!(en.contains("do not emit tool calls"), "{en}");
        assert!(zh.contains("Respond in Simplified Chinese"), "{zh}");
        assert!(
            zh.contains("approval system is handled by cosh-shell"),
            "{zh}"
        );
        assert!(zh.contains("run_shell_command"), "{zh}");
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
