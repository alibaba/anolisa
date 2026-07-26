use super::*;

#[test]
fn text_without_the_marker_carries_no_question() {
    assert_eq!(
        parse_in_band_question("Here is the plan, no question needed."),
        InBandQuestion::Absent
    );
    assert_eq!(parse_in_band_question(""), InBandQuestion::Absent);
}

#[test]
fn marked_text_with_a_valid_payload_shares_the_tool_contract() {
    let InBandQuestion::Valid(params) = parse_in_band_question(
        "Deciding.\nCOSH_QUESTION:{\"question\":\"Which branch?\",\"options\":[{\"label\":\"main\"}]}",
    ) else {
        panic!("valid payload must produce a question");
    };
    assert_eq!(params.question, "Which branch?");
    assert_eq!(params.options.len(), 1);
    assert!(params.allow_free_text, "omitted optional keeps its default");
}

/// The marker suppresses the visible assistant text, so every rejected payload
/// must be reported as a failure rather than collapsing into `Absent`.
#[test]
fn marked_text_with_an_unusable_payload_is_invalid_not_absent() {
    let cases: &[(&str, AskUserArgumentError)] = &[
        ("COSH_QUESTION:", AskUserArgumentError::EmptyArguments),
        ("COSH_QUESTION:   ", AskUserArgumentError::EmptyArguments),
        (
            "COSH_QUESTION:{\"question\":\"Which bra",
            AskUserArgumentError::InvalidJson,
        ),
        (
            "COSH_QUESTION:{\"prompt\":\"Which branch?\"}",
            AskUserArgumentError::MissingQuestion,
        ),
        (
            "COSH_QUESTION:{\"question\":null}",
            AskUserArgumentError::QuestionWrongType,
        ),
        (
            "COSH_QUESTION:{\"question\":\"Which branch?\",\"allow_free_text\":false}",
            AskUserArgumentError::NoAnswerPath,
        ),
        (
            "COSH_QUESTION:[{\"question\":\"Which branch?\"}]",
            AskUserArgumentError::RootNotObject,
        ),
    ];

    for (text, expected) in cases {
        assert_eq!(
            parse_in_band_question(text),
            InBandQuestion::Invalid(*expected),
            "payload {text:?} must be reported as invalid"
        );
    }
}

/// The failure text is what the user sees, so it must name the cause without
/// echoing a payload that may contain session content.
#[test]
fn in_band_error_names_the_code_without_echoing_the_payload() {
    let message = in_band_question_error(AskUserArgumentError::MissingQuestion);
    assert!(message.contains("code=missing_question"), "{message}");
    assert!(message.contains("COSH_QUESTION"), "{message}");
    assert!(message.contains("No question was shown"), "{message}");
}

#[test]
fn empty_arguments_are_an_empty_object_and_malformed_ones_are_errors() {
    assert_eq!(
        parse_tool_arguments("  \n"),
        Ok(serde_json::Value::Object(serde_json::Map::new()))
    );
    assert_eq!(
        parse_tool_arguments(r#"{"command":"ls"#),
        Err(ArgumentError::InvalidJson)
    );
    assert_eq!(
        parse_tool_arguments(r#"{"command":"ls"}"#),
        Ok(serde_json::json!({"command": "ls"}))
    );
}

/// Every tool declares an object root. A non-object payload parses cleanly, so
/// without this check it would pass admission and reach the tool — as `null`
/// (every field looks absent) or as arguments an MCP server never declared.
#[test]
fn parsed_arguments_that_are_not_an_object_are_rejected() {
    let cases: &[(&str, &str)] = &[
        ("null", "null"),
        ("[]", "array"),
        (r#"[{"command":"ls"}]"#, "array"),
        ("7", "number"),
        ("true", "boolean"),
        (r#""command""#, "string"),
    ];

    for (raw, shape) in cases {
        assert_eq!(
            parse_tool_arguments(raw),
            Err(ArgumentError::RootNotObject { shape }),
            "payload {raw:?} must be refused"
        );
    }
}

#[test]
fn rejection_message_carries_a_code_and_no_payload() {
    let malformed = invalid_arguments_message("shell", &ArgumentError::InvalidJson);
    assert!(malformed.contains("code=invalid_json"), "{malformed}");
    assert!(malformed.contains("shell"), "{malformed}");

    let wrong_root =
        invalid_arguments_message("shell", &ArgumentError::RootNotObject { shape: "array" });
    assert!(
        wrong_root.contains("code=arguments_not_object"),
        "{wrong_root}"
    );
    assert!(wrong_root.contains("JSON array"), "{wrong_root}");
}
