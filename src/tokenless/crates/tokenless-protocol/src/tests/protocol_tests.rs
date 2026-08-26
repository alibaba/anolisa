// Contract tests for protocol v1. The two *_roadmap_example tests pin the
// §4.1 JSON examples from the evolution roadmap draft. That document has not
// landed in-repo yet, so until it does the JSON here is the authoritative
// wire contract; once it lands, these tests become the drift guard between
// the document and the types.

#[test]
fn request_roadmap_example_parses() {
    let json = r#"{
  "protocol_version": 1,
  "content": "...",
  "agent_id": "claude-code",
  "session_id": "...",
  "tool_use_id": "...",
  "tool_name": "Bash",
  "seam": "post_tool",
  "capabilities": {
    "replace_output": true,
    "publish_retrieve_tool": true
  }
}"#;
    let req = CompressionRequest::from_json(json).expect("roadmap request example must parse");
    assert_eq!(req.protocol_version, PROTOCOL_VERSION);
    assert_eq!(req.agent_id, "claude-code");
    assert_eq!(req.tool_name.as_deref(), Some("Bash"));
    assert_eq!(req.seam, Seam::PostTool);
    assert!(req.capabilities.replace_output);
    assert!(req.capabilities.publish_retrieve_tool);
}

#[test]
fn response_roadmap_example_parses() {
    let json = r#"{
  "protocol_version": 1,
  "output": "...",
  "disposition": "applied",
  "content_type": "build_log",
  "compressor_chain": ["terminal-cleanup", "build-log"],
  "reversibility": "retrievable",
  "before_tokens": 1200,
  "after_tokens": 340,
  "stash_keys": ["0123456789abcdef01234567"]
}"#;
    let resp = CompressionResponse::from_json(json).expect("roadmap response example must parse");
    assert!(resp.is_applied());
    assert_eq!(resp.content_type.as_deref(), Some("build_log"));
    assert_eq!(resp.compressor_chain, ["terminal-cleanup", "build-log"]);
    assert_eq!(resp.reversibility, Reversibility::Retrievable);
    assert_eq!((resp.before_tokens, resp.after_tokens), (1200, 340));
    assert_eq!(resp.stash_keys, ["0123456789abcdef01234567"]);
    // The example predates the field; absence reads as the heuristic
    // estimator, the only counter that ever shipped before the field.
    assert_eq!(resp.tokenizer_id, TOKENIZER_ID);
}

#[test]
fn request_round_trips() {
    let mut req = CompressionRequest::new("hello world", "codex", Seam::PostTool);
    req.session_id = Some("s-1".into());
    req.tool_use_id = Some("tu-1".into());
    req.tool_name = Some("Bash".into());
    req.capabilities.replace_output = true;
    let parsed = CompressionRequest::from_json(&req.to_json().unwrap()).unwrap();
    assert_eq!(parsed, req);
}

#[test]
fn response_round_trips() {
    let req = CompressionRequest::new("payload", "claude-code", Seam::PreTool);
    let resp = CompressionResponse::passthrough(&req, 42);
    let parsed = CompressionResponse::from_json(&resp.to_json().unwrap()).unwrap();
    assert_eq!(parsed, resp);
}

#[test]
fn unknown_fields_are_ignored() {
    let json = r#"{
  "protocol_version": 1,
  "content": "x",
  "agent_id": "a",
  "seam": "post_tool",
  "future_optional_field": {"nested": [1, 2, 3]}
}"#;
    let req = CompressionRequest::from_json(json).expect("unknown optional fields are ignored");
    assert_eq!(req.content, "x");
}

#[test]
fn missing_optionals_take_defaults() {
    let json = r#"{"protocol_version":1,"content":"x","agent_id":"a","seam":"before_model"}"#;
    let req = CompressionRequest::from_json(json).unwrap();
    assert_eq!(req.session_id, None);
    assert_eq!(req.tool_use_id, None);
    assert_eq!(req.tool_name, None);
    assert!(!req.capabilities.replace_output);
    assert!(!req.capabilities.publish_retrieve_tool);
    assert!(!req.capabilities.replace_with_text);
}

#[test]
fn replace_with_text_defaults_false_and_parses_when_declared() {
    // Requests from adapters predating the field must keep the conservative
    // structured-slot semantics.
    let json = r#"{"protocol_version":1,"content":"x","agent_id":"a","seam":"post_tool","capabilities":{"replace_output":true}}"#;
    let req = CompressionRequest::from_json(json).unwrap();
    assert!(!req.capabilities.replace_with_text);

    let json = r#"{"protocol_version":1,"content":"x","agent_id":"a","seam":"post_tool","capabilities":{"replace_output":true,"replace_with_text":true}}"#;
    let req = CompressionRequest::from_json(json).unwrap();
    assert!(req.capabilities.replace_with_text);
}

#[test]
fn unsupported_version_beats_shape_errors() {
    // A v2 payload with a shape v1 cannot parse must still be reported as a
    // version problem, not a malformed payload.
    let json = r#"{"protocol_version":2,"body":{"parts":["..."]}}"#;
    match CompressionRequest::from_json(json) {
        Err(ProtocolError::UnsupportedVersion { found: 2 }) => {}
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
    match CompressionResponse::from_json(json) {
        Err(ProtocolError::UnsupportedVersion { found: 2 }) => {}
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn malformed_payload_is_reported() {
    let err = CompressionRequest::from_json("not json").unwrap_err();
    assert!(matches!(err, ProtocolError::Malformed(_)));
    // The structured serde error stays reachable through the source chain.
    assert!(std::error::Error::source(&err).is_some());
    // Valid version, wrong shape for the rest.
    assert!(matches!(
        CompressionRequest::from_json(r#"{"protocol_version":1,"content":7}"#),
        Err(ProtocolError::Malformed(_))
    ));
}

#[test]
fn direct_deserialization_cannot_bypass_the_version_gate() {
    // A v2 payload whose remaining fields happen to fit the v1 shape must
    // fail even through plain serde, not only through from_json.
    let json = r#"{"protocol_version":2,"content":"x","agent_id":"a","seam":"post_tool"}"#;
    let err = serde_json::from_str::<CompressionRequest>(json).unwrap_err();
    assert!(err.to_string().contains("unsupported protocol_version 2"));

    let json = r#"{"protocol_version":2,"output":"o","disposition":"applied","reversibility":"lossless","before_tokens":1,"after_tokens":1}"#;
    let err = serde_json::from_str::<CompressionResponse>(json).unwrap_err();
    assert!(err.to_string().contains("unsupported protocol_version 2"));
}

#[test]
fn passthrough_is_canonical() {
    let req = CompressionRequest::new("original", "qoder-cli", Seam::PostTool);
    let resp = CompressionResponse::passthrough(&req, 9);
    assert_eq!(resp.output, "original");
    assert_eq!(resp.disposition, Disposition::Passthrough);
    assert_eq!(resp.reversibility, Reversibility::Lossless);
    assert_eq!((resp.before_tokens, resp.after_tokens), (9, 9));
    assert!(resp.compressor_chain.is_empty());
    assert!(resp.stash_keys.is_empty());
    assert_eq!(resp.tokenizer_id, TOKENIZER_ID);
    assert!(!resp.is_applied());
}

#[test]
fn wire_format_is_stable() {
    // Locks field names and enum wire values. A failure here is a protocol
    // change and requires either a compatible optional field or a new
    // protocol_version — never a silent rename.
    let mut req = CompressionRequest::new("c", "a", Seam::PostTool);
    req.capabilities.replace_output = true;
    assert_eq!(
        req.to_json().unwrap(),
        r#"{"protocol_version":1,"content":"c","agent_id":"a","seam":"post_tool","capabilities":{"replace_output":true,"publish_retrieve_tool":false,"replace_with_text":false}}"#
    );

    let resp = CompressionResponse {
        protocol_version: PROTOCOL_VERSION,
        output: "o".into(),
        disposition: Disposition::NoSavings,
        content_type: Some("search_results".into()),
        compressor_chain: vec!["search".into()],
        reversibility: Reversibility::Unrecoverable,
        before_tokens: 10,
        after_tokens: 10,
        stash_keys: vec!["k".into()],
        tokenizer_id: TOKENIZER_ID.into(),
        diagnostic: Some("d".into()),
    };
    assert_eq!(
        resp.to_json().unwrap(),
        r#"{"protocol_version":1,"output":"o","disposition":"no_savings","content_type":"search_results","compressor_chain":["search"],"reversibility":"unrecoverable","before_tokens":10,"after_tokens":10,"stash_keys":["k"],"tokenizer_id":"heuristic-v1","diagnostic":"d"}"#
    );
}

#[test]
fn all_seams_and_dispositions_round_trip() {
    for (seam, wire) in [
        (Seam::BeforeModel, "\"before_model\""),
        (Seam::PreTool, "\"pre_tool\""),
        (Seam::PostTool, "\"post_tool\""),
        (Seam::Proxy, "\"proxy\""),
    ] {
        assert_eq!(serde_json::to_string(&seam).unwrap(), wire);
        assert_eq!(serde_json::from_str::<Seam>(wire).unwrap(), seam);
        assert_eq!(format!("\"{}\"", seam.wire_str()), wire);
    }
    for (disp, wire) in [
        (Disposition::Applied, "\"applied\""),
        (Disposition::DryRun, "\"dry_run\""),
        (Disposition::Passthrough, "\"passthrough\""),
        (Disposition::NoSavings, "\"no_savings\""),
        (
            Disposition::ReversibilityUnavailable,
            "\"reversibility_unavailable\"",
        ),
        (Disposition::Timeout, "\"timeout\""),
        (Disposition::Error, "\"error\""),
    ] {
        assert_eq!(serde_json::to_string(&disp).unwrap(), wire);
        assert_eq!(serde_json::from_str::<Disposition>(wire).unwrap(), disp);
        assert_eq!(format!("\"{}\"", disp.wire_str()), wire);
    }
    for (rev, wire) in [
        (Reversibility::Lossless, "\"lossless\""),
        (Reversibility::Retrievable, "\"retrievable\""),
        (Reversibility::Unrecoverable, "\"unrecoverable\""),
    ] {
        assert_eq!(serde_json::to_string(&rev).unwrap(), wire);
        assert_eq!(serde_json::from_str::<Reversibility>(wire).unwrap(), rev);
    }
}
