use std::sync::{Arc, Mutex};

use super::cosh_core::CoshCoreAdapter;
use super::AgentAdapter;
use crate::types::{
    AgentEvent, AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, OutputRefs,
};

fn test_request() -> AgentRequest {
    AgentRequest {
        id: "test".to_string(),
        session_id: "sess".to_string(),
        command_block: CommandBlock {
            id: "blk".to_string(),
            session_id: "sess".to_string(),
            command: "echo test".to_string(),
            origin: Default::default(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 0,
            ended_at_ms: 0,
            duration_ms: 0,
            exit_code: 1,
            status: CommandStatus::Failed,
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        },
        context_blocks: vec![],
        context_hints: vec![],
        user_input: Some("test".to_string()),
        findings: vec![],
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    }
}

fn test_adapter() -> CoshCoreAdapter {
    CoshCoreAdapter {
        program: "cosh-core".to_string(),
        allow_model_call: false,
        session_id: Arc::new(Mutex::new(None)),
        session_cwd: Arc::new(Mutex::new(None)),
    }
}

#[test]
fn prepare_invocation_headless_flag() {
    let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert_eq!(inv.program, "cosh-core");
    assert!(inv.args.contains(&"--headless".to_string()));
    assert!(inv
        .args
        .contains(&"--enable-shell-evidence-tool".to_string()));
}

#[test]
fn prepare_invocation_approval_modes() {
    let recommend = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Recommend);
    assert!(recommend.args.contains(&"strict".to_string()));

    let auto = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert!(auto.args.contains(&"auto".to_string()));

    let trust = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Trust);
    assert!(trust.args.contains(&"trust".to_string()));
}

#[test]
fn prepare_invocation_prompt_leaves_shell_tool_trigger_to_cosh_core() {
    let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);

    assert!(inv
        .prompt
        .contains("Handle this natural-language shell prompt request"));
    assert!(!inv.prompt.contains("cosh-shell Agent contract"));
    assert!(!inv
        .prompt
        .contains("Always emit a provider permission request"));
    assert!(!inv.prompt.contains("cosh-core adapter compatibility"));
}

#[test]
fn prepare_invocation_prompt_uses_shell_output_tool_mode() {
    let mut request = test_request();
    let mut context = request.command_block.clone();
    context.id = "cmd-1".to_string();
    context.session_id = "session-1".to_string();
    context.exit_code = 0;
    context.status = CommandStatus::Completed;
    context.output.terminal_output_ref = Some("/tmp/cosh-output.txt".to_string());
    context.output.terminal_output_bytes = 42;
    request.context_blocks = vec![context];

    let inv = test_adapter().prepare_invocation(&request, CoshApprovalMode::Auto);

    assert!(inv.prompt.contains("cosh_shell_evidence"), "{}", inv.prompt);
    assert!(
        inv.prompt.contains("action=list_commands"),
        "{}",
        inv.prompt
    );
    assert!(inv.prompt.contains("action=read_output"), "{}", inv.prompt);
    assert!(
        inv.prompt.contains("Use current tool results first"),
        "{}",
        inv.prompt
    );
    assert!(
        inv.prompt
            .contains("Use read_output only for older shell ledger output"),
        "{}",
        inv.prompt
    );
    assert!(
        inv.prompt.contains("activity recaps or command lists"),
        "{}",
        inv.prompt
    );
    assert!(
        inv.prompt.contains("output_available=false"),
        "{}",
        inv.prompt
    );
    assert!(inv.prompt.contains("output_bytes=0"), "{}", inv.prompt);
    assert!(
        inv.prompt
            .contains("call cosh_shell_evidence with action=list_commands"),
        "{}",
        inv.prompt
    );
    assert!(!inv.prompt.contains("```cosh-request"), "{}", inv.prompt);
    assert!(
        !inv.prompt.contains("```cosh-request\noutput"),
        "{}",
        inv.prompt
    );
}

#[test]
fn prepare_invocation_prompt_suppresses_shell_output_requests_in_recommend_mode() {
    let mut request = test_request();
    let mut context = request.command_block.clone();
    context.id = "cmd-1".to_string();
    context.session_id = "session-1".to_string();
    context.output.terminal_output_ref = Some("/tmp/cosh-output.txt".to_string());
    context.output.terminal_output_bytes = 42;
    request.context_blocks = vec![context];

    let inv = test_adapter().prepare_invocation(&request, CoshApprovalMode::Recommend);

    assert!(
        inv.prompt
            .contains("do not request shell output automatically"),
        "{}",
        inv.prompt
    );
    assert!(
        !inv.prompt.contains("cosh_shell_evidence"),
        "{}",
        inv.prompt
    );
    assert!(!inv.prompt.contains("```cosh-request"), "{}", inv.prompt);
}

#[test]
fn prepare_invocation_session_resume() {
    let adapter = CoshCoreAdapter {
        program: "cosh-core".to_string(),
        allow_model_call: false,
        session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
        session_cwd: Arc::new(Mutex::new(Some("/tmp".to_string()))),
    };
    let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert!(inv.args.contains(&"--resume".to_string()));
    assert!(inv.args.contains(&"prev-sess".to_string()));
}

#[test]
fn prepare_invocation_does_not_resume_across_cwd_scope() {
    let adapter = CoshCoreAdapter {
        program: "cosh-core".to_string(),
        allow_model_call: false,
        session_id: Arc::new(Mutex::new(Some("prev-sess".to_string()))),
        session_cwd: Arc::new(Mutex::new(Some("/other".to_string()))),
    };
    let inv = adapter.prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert!(!inv.args.contains(&"--resume".to_string()));
    assert!(!inv.args.contains(&"prev-sess".to_string()));
}

#[test]
fn capabilities_match_expected() {
    let adapter = test_adapter();
    let caps = adapter.capabilities();
    assert!(caps.text_stream);
    assert!(caps.session_resume);
    assert!(caps.tool_intent);
    assert!(caps.user_question);
    assert!(caps.cancellable);
    assert!(caps.control_protocol);
}

#[test]
fn stream_parser_uses_neutral_status_messages() {
    let script =
        std::env::temp_dir().join(format!("cosh-tui-neutral-status-{}.sh", std::process::id()));
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hidden reasoning"}}}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"s","is_error":false,"result":"done"}'
"#,
    )
    .expect("write mock cosh-tui");
    let mut permissions = std::fs::metadata(&script)
        .expect("mock cosh-tui metadata")
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("chmod mock cosh-tui");

    let adapter = CoshCoreAdapter {
        program: script.to_string_lossy().to_string(),
        allow_model_call: true,
        session_id: Arc::new(Mutex::new(None)),
        session_cwd: Arc::new(Mutex::new(None)),
    };
    let mut events = Vec::new();
    let result = adapter.run_stream(&test_request(), &mut |event| {
        events.push(event);
        Ok(())
    });
    let _ = std::fs::remove_file(&script);
    result.expect("run mock cosh-tui");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::StatusChanged { phase, message, .. }
            if phase == "thinking" && message == "thinking"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AgentCompleted { summary, .. } if summary == "analysis completed"
    )));
    let debug = format!("{events:?}");
    assert!(!debug.contains("claude"), "{debug}");
    assert!(!debug.contains("co thinking"), "{debug}");
}
