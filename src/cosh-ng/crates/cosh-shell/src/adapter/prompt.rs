use crate::evidence::{
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
