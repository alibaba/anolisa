use std::sync::{Arc, Mutex};

use super::cosh_tui::CoshTuiAdapter;
use super::AgentAdapter;
use crate::types::{
    AgentMode, AgentRequest, CommandBlock, CommandStatus, CoshApprovalMode, OutputRefs,
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

fn test_adapter() -> CoshTuiAdapter {
    CoshTuiAdapter {
        program: "cosh-tui".to_string(),
        allow_model_call: false,
        session_id: Arc::new(Mutex::new(None)),
        session_cwd: Arc::new(Mutex::new(None)),
    }
}

#[test]
fn prepare_invocation_headless_flag() {
    let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);
    assert_eq!(inv.program, "cosh-tui");
    assert!(inv.args.contains(&"--headless".to_string()));
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
fn prepare_invocation_prompt_leaves_shell_tool_trigger_to_cosh_tui() {
    let inv = test_adapter().prepare_invocation(&test_request(), CoshApprovalMode::Auto);

    assert!(inv
        .prompt
        .contains("Handle this natural-language shell prompt request"));
    assert!(!inv.prompt.contains("cosh-shell Agent contract"));
    assert!(!inv
        .prompt
        .contains("Always emit a provider permission request"));
    assert!(!inv.prompt.contains("cosh-tui adapter compatibility"));
}

#[test]
fn prepare_invocation_session_resume() {
    let adapter = CoshTuiAdapter {
        program: "cosh-tui".to_string(),
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
    let adapter = CoshTuiAdapter {
        program: "cosh-tui".to_string(),
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
