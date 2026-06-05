use std::os::unix::fs::PermissionsExt;

use cosh_shell::{
    adapter_for_kind, agent_request_after_confirmation, agent_request_confirmed_by_events,
    build_command_blocks, findings_from_blocks, govern_agent_events, interventions_from_findings,
    render_transcript, AdapterKind, AgentAdapter, AgentEvent, ClaudeCodeAdapter, CommandStatus,
    FakeAgentAdapter, FindingKind, GovernanceDecision, InterventionDecision, Policy,
    QuestionSelectionMode, QwenCliAdapter, ShellEvent, ShellEventKind,
};

fn failed_command_events() -> Vec<ShellEvent> {
    vec![
        ShellEvent::user_input_intercepted("session-1", "/explain why this failed"),
        ShellEvent::command_started("session-1", "cmd-1", "missing-command", "/work", 1000),
        ShellEvent::command_finished(
            ShellEventKind::CommandFailed,
            "session-1",
            "cmd-1",
            127,
            1037,
            "terminal://session-1/cmd-1",
        ),
    ]
}

#[test]
fn ledger_builds_failed_command_block_from_shell_events() {
    let ledger = build_command_blocks(&failed_command_events());

    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert_eq!(ledger.blocks.len(), 1);
    let block = &ledger.blocks[0];
    assert_eq!(block.id, "cmd-1");
    assert_eq!(block.command, "missing-command");
    assert_eq!(block.cwd, "/work");
    assert_eq!(block.exit_code, 127);
    assert_eq!(block.status, CommandStatus::Failed);
    assert_eq!(block.duration_ms, 37);
    assert_eq!(
        block.output.terminal_output_ref.as_deref(),
        Some("terminal://session-1/cmd-1")
    );
}

#[test]
fn parser_suggests_before_agent_request_is_created() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let interventions = interventions_from_findings(&findings);

    assert!(findings
        .iter()
        .any(|finding| finding.kind == FindingKind::NonZeroExit));
    assert!(findings
        .iter()
        .any(|finding| finding.kind == FindingKind::CommandNotFound));
    assert!(interventions.iter().all(|intervention| {
        intervention.command_block_id == "cmd-1"
            && intervention.decision == InterventionDecision::Suggest
    }));
    assert!(
        agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, false)
            .is_none()
    );
}

#[test]
fn failed_command_does_not_confirm_agent_without_intercepted_user_request() {
    let plain_failure = vec![
        ShellEvent::command_started("session-1", "cmd-1", "missing-command", "/work", 1000),
        ShellEvent::command_finished(
            ShellEventKind::CommandFailed,
            "session-1",
            "cmd-1",
            127,
            1037,
            "terminal://session-1/cmd-1",
        ),
    ];

    assert!(!agent_request_confirmed_by_events(&plain_failure));
    assert!(agent_request_confirmed_by_events(&failed_command_events()));
}

#[test]
fn natural_language_intercept_does_not_confirm_failed_command_analysis() {
    let natural_language_then_failure = vec![
        ShellEvent::user_input_intercepted("session-1", "please explain why this failed"),
        ShellEvent::command_started("session-1", "cmd-1", "missing-command", "/work", 1000),
        ShellEvent::command_finished(
            ShellEventKind::CommandFailed,
            "session-1",
            "cmd-1",
            127,
            1037,
            "terminal://session-1/cmd-1",
        ),
    ];

    assert!(!agent_request_confirmed_by_events(
        &natural_language_then_failure
    ));
}

#[test]
fn fake_agent_and_governance_form_recommend_only_loop() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
        .expect("confirmed request");

    let agent_events = FakeAgentAdapter.run(&request).expect("fake adapter");
    assert!(agent_events.iter().any(|event| matches!(
        event,
        AgentEvent::Recommendation {
            auto_execute: false,
            ..
        }
    )));

    let governed = govern_agent_events(&agent_events, &Policy::default());
    assert_eq!(governed.events.len(), agent_events.len());
    assert_eq!(governed.audit.len(), agent_events.len());
    assert!(governed.events.iter().all(|event| !event.auto_execute));

    let transcript = render_transcript(
        &ledger.blocks[0],
        &findings,
        &interventions_from_findings(&findings),
        &governed.events,
    );
    assert!(transcript
        .iter()
        .any(|line| line.contains("Display-only: recommendations are not executed automatically")));
}

#[test]
fn fake_agent_streams_events_through_adapter_boundary() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
        .expect("confirmed request");

    let mut streamed = Vec::new();
    FakeAgentAdapter
        .run_stream(&request, &mut |event| {
            streamed.push(event);
            Ok(())
        })
        .expect("fake stream adapter");

    assert!(matches!(
        streamed.first(),
        Some(AgentEvent::StatusChanged { phase, .. }) if phase == "analyzing"
    ));
    assert!(streamed
        .iter()
        .any(|event| matches!(event, AgentEvent::TextDelta { .. })));
    assert!(matches!(
        streamed.last(),
        Some(AgentEvent::AgentCompleted { .. })
    ));
}

#[test]
fn governance_rejects_tool_calls_and_agent_actions() {
    let events = vec![
        AgentEvent::StatusChanged {
            run_id: "run-1".to_string(),
            phase: "requesting".to_string(),
            message: "waiting for backend".to_string(),
        },
        AgentEvent::ToolCall {
            run_id: "run-1".to_string(),
            name: "shell".to_string(),
            input: "rm -rf /tmp/example".to_string(),
        },
        AgentEvent::Action {
            run_id: "run-1".to_string(),
            command: "sudo reboot".to_string(),
        },
        AgentEvent::Recommendation {
            run_id: "run-1".to_string(),
            summary: "Try a safer check".to_string(),
            commands: vec!["systemctl status demo".to_string()],
            auto_execute: true,
        },
        AgentEvent::SkillLoadCompleted {
            run_id: "run-1".to_string(),
            skill: "service-debug".to_string(),
            summary: "loaded service debugging guidance".to_string(),
        },
        AgentEvent::ToolOutputDelta {
            run_id: "run-1".to_string(),
            tool_id: "tool-1".to_string(),
            stream: "stdout".to_string(),
            text: "line 1".to_string(),
        },
        AgentEvent::AgentCancelled {
            run_id: "run-1".to_string(),
            reason: "user requested cancellation".to_string(),
        },
    ];

    let governed = govern_agent_events(&events, &Policy::default());
    assert_eq!(governed.events[0].decision, GovernanceDecision::Display);
    assert_eq!(governed.events[1].decision, GovernanceDecision::Rejected);
    assert_eq!(governed.events[2].decision, GovernanceDecision::Rejected);
    assert_eq!(governed.events[3].decision, GovernanceDecision::Degraded);
    assert_eq!(governed.events[4].decision, GovernanceDecision::Display);
    assert_eq!(governed.events[5].decision, GovernanceDecision::Display);
    assert_eq!(governed.events[6].decision, GovernanceDecision::Display);
    assert!(governed.events.iter().all(|event| !event.auto_execute));
    assert!(governed.events[0]
        .display_text
        .contains("Status: requesting"));
    assert!(governed.events[1]
        .display_text
        .contains("Approval required: Bash command"));
    assert!(governed.events[1]
        .display_text
        .contains("Blocked: user approval required"));
    assert!(governed.events[2]
        .display_text
        .contains("Approval required: Shell command"));
    assert!(governed
        .events
        .iter()
        .all(|event| !event.display_text.contains("Decision: blocked by")));
    assert!(governed.events[4]
        .display_text
        .contains("Skill loaded: service-debug"));
    assert!(governed.events[5]
        .display_text
        .contains("Tool output: tool-1 stdout"));
    assert!(governed.events[6].display_text.contains("Agent cancelled"));
}

#[test]
fn cli_adapters_prepare_safe_non_intrusive_invocations() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
        .expect("confirmed request");

    let claude = ClaudeCodeAdapter::default().prepare_invocation(&request);
    assert_eq!(claude.program, "claude");
    assert!(claude.args.contains(&"--print".to_string()));
    assert!(claude.args.contains(&"--output-format".to_string()));
    assert!(claude.args.contains(&"stream-json".to_string()));
    assert!(claude.args.contains(&"--verbose".to_string()));
    assert!(claude
        .args
        .contains(&"--include-partial-messages".to_string()));
    assert!(claude.args.contains(&"--permission-mode".to_string()));
    assert!(claude.args.contains(&"plan".to_string()));
    assert!(claude.args.contains(&"--tools".to_string()));
    assert!(claude.args.contains(&"default".to_string()));
    assert!(!claude.args.contains(&"--allowedTools".to_string()));
    assert!(!claude
        .args
        .contains(&"--no-session-persistence".to_string()));
    assert!(!claude
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));
    assert!(claude.prompt.contains("Do not execute commands."));
    let qwen = QwenCliAdapter::default().prepare_invocation(&request);
    assert_eq!(qwen.program, "qwen");
    assert!(qwen.args.contains(&"--approval-mode".to_string()));
    assert!(qwen.args.contains(&"plan".to_string()));
}

#[test]
fn natural_language_prompt_guides_tool_and_question_intents() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let mut request =
        agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
            .expect("confirmed request");
    request.user_input = Some("执行 ps aux 看一下".to_string());

    let claude = ClaudeCodeAdapter::default().prepare_invocation(&request);

    assert!(claude.prompt.contains("request the Bash tool"));
    assert!(claude.prompt.contains("request AskUserQuestion"));
    assert!(claude.prompt.contains("Do not execute commands."));
}

#[test]
fn continuation_prompts_do_not_reenter_generic_shell_request_mode() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let mut request =
        agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
            .expect("confirmed request");

    request.user_input =
        Some("Answer to pending Agent question: 你喜欢什么颜色？\nUser answer: 白色".to_string());
    let question_prompt = ClaudeCodeAdapter::default()
        .prepare_invocation(&request)
        .prompt;
    assert!(question_prompt.contains("Continue the same Shell-first Agent session"));
    assert!(question_prompt.contains("Do not ask the same question again"));
    assert!(!question_prompt.contains("Return explanation and recommended next commands only"));

    request.user_input = Some(
        "Tool result for approved request req-1\nTool: tool Bash\nCommand: pwd\nStatus: executed\nExit code: 0\nReason: ok\nStdout:\n/work\nStderr:\n".to_string(),
    );
    let tool_prompt = ClaudeCodeAdapter::default()
        .prepare_invocation(&request)
        .prompt;
    assert!(tool_prompt.contains("approved tool result"));
    assert!(tool_prompt.contains("Analyze only the result below"));
    assert!(tool_prompt.contains("pre-approval prose"));
    assert!(tool_prompt.contains("do not continue an earlier recommendation list"));
    assert!(!tool_prompt.contains("Return explanation and recommended next commands only"));

    request.user_input = Some(
        "Approval result for request req-1\nTool: tool Bash\nCommand: pwd\nDecision: denied by user\nNo command ran.".to_string(),
    );
    let approval_prompt = ClaudeCodeAdapter::default()
        .prepare_invocation(&request)
        .prompt;
    assert!(approval_prompt.contains("approval decision"));
    assert!(approval_prompt.contains("No shell command ran"));
    assert!(!approval_prompt.contains("Return explanation and recommended next commands only"));
}

#[test]
fn fake_and_qwen_use_same_agent_adapter_boundary() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
        .expect("confirmed request");

    let fake_events = adapter_for_kind(AdapterKind::Fake)
        .run(&request)
        .expect("fake adapter");
    let qwen_events = adapter_for_kind(AdapterKind::QwenCli)
        .run(&request)
        .expect("qwen adapter");

    assert!(fake_events.iter().any(|event| matches!(
        event,
        AgentEvent::Recommendation {
            auto_execute: false,
            ..
        }
    )));
    assert!(qwen_events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. }
            if text.contains("qwen --approval-mode plan <prompt>")
    )));

    let governed = govern_agent_events(&qwen_events, &Policy::default());
    assert_eq!(governed.events.len(), qwen_events.len());
    assert_eq!(governed.audit.len(), qwen_events.len());
    assert!(governed.events.iter().all(|event| !event.auto_execute));
}

#[test]
fn adapter_capabilities_are_provider_neutral() {
    let fake = adapter_for_kind(AdapterKind::Fake).capabilities();
    let claude = adapter_for_kind(AdapterKind::ClaudeCode).capabilities();
    let qwen = adapter_for_kind(AdapterKind::QwenCli).capabilities();

    assert!(fake.text_stream);
    assert!(fake.tool_intent);
    assert!(fake.user_question);
    assert!(!fake.session_resume);

    assert!(claude.text_stream);
    assert!(claude.thinking_stream);
    assert!(claude.session_resume);
    assert!(claude.tool_intent);
    assert!(claude.user_question);
    assert!(claude.cancellable);

    assert!(!qwen.session_resume);
    assert!(!qwen.tool_intent);
    assert!(!qwen.user_question);
}

#[test]
fn claude_code_adapter_is_dry_run_by_default_and_governed() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
        .expect("confirmed request");

    let claude_events = adapter_for_kind(AdapterKind::ClaudeCode)
        .run(&request)
        .expect("claude adapter");

    assert!(claude_events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. }
            if text.contains("claude --print") && text.contains("--permission-mode plan")
    )));
    assert!(claude_events.iter().any(|event| matches!(
        event,
        AgentEvent::AgentCompleted { summary, .. }
            if summary.contains("dry-run completed without model call")
    )));

    let governed = govern_agent_events(&claude_events, &Policy::default());
    assert_eq!(governed.events.len(), claude_events.len());
    assert_eq!(governed.audit.len(), claude_events.len());
    assert!(governed.events.iter().all(|event| !event.auto_execute));
}

#[test]
fn claude_code_adapter_streams_question_options_from_input_json_delta() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let mut request =
        agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
            .expect("confirmed request");
    request.user_input = Some("问我喜欢什么颜色".to_string());

    let script = std::env::temp_dir().join(format!(
        "cosh-shell-claude-stream-question-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"session-stream"}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-ask","name":"AskUserQuestion","input":{}}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"questions\":[{\"question\":\"你喜欢什么颜色？\",\"header\":\"颜色\",\"options\":[{\"label\":\"白色\"},{\"label\":\"黑色\"},{\"label\":\"蓝色\"}],\"multiSelect\":false}]}"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_stop","index":0}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"session-stream"}'
"#,
    )
    .expect("write fake claude script");
    let mut permissions = std::fs::metadata(&script)
        .expect("fake claude script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("fake claude script permissions");

    let mut adapter = ClaudeCodeAdapter::default().with_model_call(true);
    adapter.program = script.to_string_lossy().to_string();
    let mut streamed = Vec::new();
    let result = adapter.run_stream(&request, &mut |event| {
        streamed.push(event);
        Ok(())
    });
    let _ = std::fs::remove_file(&script);
    result.expect("stream fake claude adapter");

    assert!(streamed.iter().any(|event| matches!(
        event,
        AgentEvent::UserQuestion {
            question,
            options,
            allow_free_text,
            selection_mode,
            ..
        } if question == "你喜欢什么颜色？"
            && options == &vec![
                "白色".to_string(),
                "黑色".to_string(),
                "蓝色".to_string(),
            ]
            && *allow_free_text
            && *selection_mode == QuestionSelectionMode::Single
    )));
    assert!(streamed.iter().any(|event| matches!(
        event,
        AgentEvent::AgentCompleted { summary, .. }
            if summary.contains("claude code analysis completed")
    )));
}

#[test]
fn claude_code_adapter_reads_question_options_from_permission_request() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let mut request =
        agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
            .expect("confirmed request");
    request.user_input = Some("问我喜欢什么颜色".to_string());

    let script = std::env::temp_dir().join(format!(
        "cosh-shell-claude-permission-question-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"session-permission"}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-ask","name":"AskUserQuestion","input":{}}]}}'
printf '%s\n' '{"event":"permission_request","toolName":"AskUserQuestion","input":{"questions":[{"question":"你喜欢什么颜色？","header":"颜色","options":[{"label":"白色"},{"label":"黑色"},{"label":"蓝色"}],"multiSelect":false}]},"permissionLevel":null}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"session-permission"}'
"#,
    )
    .expect("write fake claude script");
    let mut permissions = std::fs::metadata(&script)
        .expect("fake claude script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("fake claude script permissions");

    let mut adapter = ClaudeCodeAdapter::default().with_model_call(true);
    adapter.program = script.to_string_lossy().to_string();
    let mut streamed = Vec::new();
    let result = adapter.run_stream(&request, &mut |event| {
        streamed.push(event);
        Ok(())
    });
    let _ = std::fs::remove_file(&script);
    result.expect("stream fake claude adapter");

    assert!(streamed.iter().any(|event| matches!(
        event,
        AgentEvent::UserQuestion {
            question,
            options,
            allow_free_text,
            selection_mode,
            ..
        } if question == "你喜欢什么颜色？"
            && options == &vec![
                "白色".to_string(),
                "黑色".to_string(),
                "蓝色".to_string(),
            ]
            && *allow_free_text
            && *selection_mode == QuestionSelectionMode::Single
    )));
    assert_eq!(
        streamed
            .iter()
            .filter(|event| matches!(event, AgentEvent::UserQuestion { .. }))
            .count(),
        1
    );
}

#[test]
fn claude_code_adapter_reuses_stream_session_id_on_next_invocation() {
    let ledger = build_command_blocks(&failed_command_events());
    let findings = findings_from_blocks(&ledger.blocks);
    let request = agent_request_after_confirmation("session-1", &ledger.blocks[0], &findings, true)
        .expect("confirmed request");

    let script = std::env::temp_dir().join(format!(
        "cosh-shell-claude-resume-session-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"session-stream"}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first response"}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"session-stream"}'
"#,
    )
    .expect("write fake claude script");
    let mut permissions = std::fs::metadata(&script)
        .expect("fake claude script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("fake claude script permissions");

    let mut adapter = ClaudeCodeAdapter::default().with_model_call(true);
    adapter.program = script.to_string_lossy().to_string();
    let mut streamed = Vec::new();
    let result = adapter.run_stream(&request, &mut |event| {
        streamed.push(event);
        Ok(())
    });
    let _ = std::fs::remove_file(&script);
    result.expect("stream fake claude adapter");

    let next_invocation = adapter.prepare_invocation(&request);
    let resume_at = next_invocation
        .args
        .iter()
        .position(|arg| arg == "--resume")
        .expect("resume flag");
    assert_eq!(
        next_invocation.args.get(resume_at + 1).map(String::as_str),
        Some("session-stream")
    );
}
