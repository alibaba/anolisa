use super::prompt::{
    prompt_from_request, provider_prompt_contract, provider_prompt_contract_for_language,
};
use crate::types::{
    AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, FindingSeverity,
    HookFinding, OutputRefs,
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
    assert!(prompt.contains("Runtime context hints:"), "{prompt}");
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
        prompt.contains("Treat these as routing/context hints only"),
        "{prompt}"
    );
}

#[test]
fn prompt_includes_bounded_health_context_without_local_paths() {
    let request = AgentRequest {
        id: "agent-request-health-1".to_string(),
        session_id: "session-1".to_string(),
        command_block: command_block("input-1", "分析一下这台机器内存风险", 0, None),
        context_blocks: Vec::new(),
        context_hints: vec![
            "health_scan scan_id=health-1 overall_severity=warning facts=[memory.available_ratio:memory.available_ratio=0.080] findings=[J06:warning:HealthFindingMemoryAvailableLow:evidence=memory.available_ratio] bounded_facts_only=true no_collector_stdout=true"
                .to_string(),
        ],
        user_input: Some("分析一下这台机器内存风险".to_string()),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    };

    let prompt = prompt_from_request(&request);

    assert!(prompt.contains("Runtime context hints:"), "{prompt}");
    assert!(prompt.contains("scan_id=health-1"), "{prompt}");
    assert!(
        prompt.contains("HealthFindingMemoryAvailableLow"),
        "{prompt}"
    );
    assert!(
        prompt.contains("evidence=memory.available_ratio"),
        "{prompt}"
    );
    assert!(prompt.contains("bounded_facts_only=true"), "{prompt}");
    assert!(!prompt.contains("journalctl -k"), "{prompt}");
    assert!(!prompt.contains("dmesg"), "{prompt}");
    assert!(!prompt.contains("/tmp/cosh"), "{prompt}");
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
            hook_id: "memory-pressure".to_string(),
            severity: FindingSeverity::Warning,
            title: "Memory pressure detected".to_string(),
            description: "Available memory is low".to_string(),
            suggestion: "Use memory-analysis".to_string(),
            skill: Some("memory-analysis".to_string()),
            cli_hint: None,
            context_refs: Vec::new(),
        }),
        recommended_skill: None,
    };

    let prompt = prompt_from_request(&request);
    assert!(
        prompt.contains("Hook finding: Memory pressure detected"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Description: Available memory is low"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Recommended skill: memory-analysis"),
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
            severity: FindingSeverity::Warning,
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
    assert!(hook.contains("Runtime context hints:"), "{hook}");
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
    assert!(
        prompt.contains("do not request shell output automatically"),
        "{prompt}"
    );
    assert!(!prompt.contains("```cosh-request"), "{prompt}");
    assert!(!prompt.contains("cosh_shell_evidence"), "{prompt}");
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
        origin: Default::default(),
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
