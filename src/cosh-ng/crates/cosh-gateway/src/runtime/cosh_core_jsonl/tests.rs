//! Focused private cosh-core JSONL codec tests.

use super::*;
use serde_json::Value;

fn initialized_codec() -> CoshCoreJsonlCodec {
    let mut codec = CoshCoreJsonlCodec::new("gateway-init-1", 4096).unwrap();
    codec.initialize_frame(false).unwrap();
    let response = br#"{"type":"control_response","response":{"subtype":"success","request_id":"gateway-init-1","response":{"subtype":"initialize","protocol_version":1,"capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true,"can_handle_shell_evidence_tool":false,"can_handle_approval_receipt":true}}}}"#;
    assert!(matches!(
        codec.decode_frame(response).unwrap(),
        CoshCoreObservation::Initialized(_)
    ));
    codec
}

#[test]
fn initialize_is_explicitly_private_version_one() {
    let mut codec = CoshCoreJsonlCodec::new("gateway-init-1", 4096).unwrap();

    let frame = codec.initialize_frame(false).unwrap();
    let value: Value = serde_json::from_str(frame.trim()).unwrap();

    assert_eq!(value["type"], "control_request");
    assert_eq!(value["request"]["subtype"], "initialize");
    assert_eq!(value["request"]["protocol_version"], 1);
    assert_eq!(value["request"]["fire_session_start"], false);
    assert_eq!(codec.phase(), CoshCoreProtocolPhase::AwaitingInitialize);
}

#[test]
fn initialization_requires_exact_version_and_correlation() {
    let mut codec = CoshCoreJsonlCodec::new("expected", 4096).unwrap();
    codec.initialize_frame(true).unwrap();
    let mismatched = br#"{"type":"control_response","response":{"subtype":"success","request_id":"other","response":{"subtype":"initialize","protocol_version":1,"capabilities":{}}}}"#;

    assert!(matches!(
        codec.decode_frame(mismatched),
        Err(CoshCoreCodecError::InitializeCorrelationMismatch)
    ));

    let wrong_version = br#"{"type":"control_response","response":{"subtype":"success","request_id":"expected","response":{"subtype":"initialize","protocol_version":2,"capabilities":{}}}}"#;
    assert!(matches!(
        codec.decode_frame(wrong_version),
        Err(CoshCoreCodecError::InitializeVersionMismatch {
            required: 1,
            actual: 2
        })
    ));
}

#[test]
fn auth_bootstrap_is_only_control_request_allowed_before_ready() {
    let mut codec = CoshCoreJsonlCodec::new("init", 4096).unwrap();
    codec.initialize_frame(true).unwrap();
    let auth = br#"{"type":"control_request","request_id":"auth-1","request":{"subtype":"auth_required","reason":"not_configured","providers":[]}}"#;

    assert!(matches!(
        codec.decode_frame(auth).unwrap(),
        CoshCoreObservation::ControlRequest(CoshCoreControlRequestEnvelope {
            request: CoshCoreControlRequest::AuthRequired { .. },
            ..
        })
    ));
    assert_eq!(codec.phase(), CoshCoreProtocolPhase::AwaitingInitialize);
}

#[test]
fn result_and_eof_produce_one_terminal_observation() {
    let mut codec = initialized_codec();
    let result = br#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"provider-session"}"#;

    assert!(matches!(
        codec.decode_frame(result).unwrap(),
        CoshCoreObservation::Result(CoshCoreResult {
            is_error: false,
            ..
        })
    ));
    assert_eq!(codec.phase(), CoshCoreProtocolPhase::Terminal);
    assert_eq!(codec.finish_stdout(), None);
    assert!(matches!(
        codec.decode_frame(result),
        Err(CoshCoreCodecError::OutputAfterTerminal)
    ));
}

#[test]
fn eof_before_result_is_synthetic_terminal_once() {
    let mut codec = initialized_codec();

    assert_eq!(
        codec.finish_stdout(),
        Some(CoshCoreObservation::ProtocolEndedWithoutResult)
    );
    assert_eq!(codec.finish_stdout(), None);
}

#[test]
fn user_mapping_uses_provider_session_without_gateway_identity() {
    let codec = initialized_codec();
    let frame = codec
        .user_frame(&CoshCoreUserTurn {
            content: "diagnose".to_string(),
            provider_session_id: Some("provider-session".to_string()),
            raw_user_input: Some("diagnose".to_string()),
            shell_context: None,
        })
        .unwrap();
    let value: Value = serde_json::from_str(frame.trim()).unwrap();

    assert_eq!(value["type"], "user");
    assert_eq!(value["session_id"], "provider-session");
    assert_eq!(value["message"]["role"], "user");
}

#[test]
fn duplicate_initialize_response_is_rejected_after_readiness() {
    let mut codec = initialized_codec();
    let duplicate = br#"{"type":"control_response","response":{"subtype":"success","request_id":"gateway-init-1","response":{"subtype":"initialize","protocol_version":1,"capabilities":{}}}}"#;

    assert!(matches!(
        codec.decode_frame(duplicate),
        Err(CoshCoreCodecError::DuplicateInitializeResponse)
    ));
}

#[test]
fn user_output_accepts_only_typed_tool_results() {
    let mut codec = initialized_codec();
    let invalid = br#"{"type":"user","session_id":"provider-session","message":{"content":[{"type":"text","tool_use_id":"tool-1","is_error":false,"content":"not a tool result"}]}}"#;

    assert!(matches!(
        codec.decode_frame(invalid),
        Err(CoshCoreCodecError::Malformed(_))
    ));
}

#[test]
fn user_output_maps_typed_tool_result() {
    let mut codec = initialized_codec();
    let output = br#"{"type":"user","session_id":"provider-session","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","is_error":false,"content":"done"}]}}"#;

    let observation = codec.decode_frame(output).unwrap();
    assert_eq!(
        observation,
        CoshCoreObservation::ToolResults {
            provider_session_id: "provider-session".to_string(),
            results: vec![CoshCoreToolResult {
                tool_use_id: "tool-1".to_string(),
                is_error: false,
                content: "done".to_string(),
            }],
        }
    );
}

#[test]
fn oversized_output_is_rejected_before_json_allocation() {
    let mut codec = CoshCoreJsonlCodec::new("init", 8).unwrap();
    codec.initialize_frame(true).unwrap_err();
    assert_eq!(codec.phase(), CoshCoreProtocolPhase::Created);
    assert!(matches!(
        codec.decode_frame(b"123456789"),
        Err(CoshCoreCodecError::FrameTooLarge { limit: 8 })
    ));
}
