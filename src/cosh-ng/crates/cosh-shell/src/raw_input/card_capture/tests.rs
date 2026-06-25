use super::{CardInputState, RawInputCapture, RawInputEvent};

#[test]
fn question_capture_custom_option_waits_for_text_before_submit() {
    let capture = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 2,
        allow_free_text: true,
        multiple: false,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b"\t\t\n"),
        vec![
            RawInputEvent::CardFocus("q-1".to_string(), 1),
            RawInputEvent::CardFocus("q-1".to_string(), 2),
        ]
    );
    assert_eq!(
        state.consume(&capture, "红色\n".as_bytes()),
        vec![
            RawInputEvent::CardInput("q-1".to_string(), "红色".to_string()),
            RawInputEvent::CardAnswer("红色".to_string())
        ]
    );
}

#[test]
fn question_capture_ignores_removed_answer_slash() {
    let capture = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 2,
        allow_free_text: true,
        multiple: false,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(state.consume(&capture, b"/answer Blue\n"), vec![]);
    assert_eq!(
        state.consume(&capture, b"\x1b[C\n"),
        vec![
            RawInputEvent::CardFocus("q-1".to_string(), 1),
            RawInputEvent::CardAnswer("2".to_string())
        ]
    );
}

#[test]
fn approval_capture_ignores_removed_decision_slashes() {
    let capture = RawInputCapture::Approval {
        id: "req-1".to_string(),
        is_hook: false,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(state.consume(&capture, b"/approve\n"), vec![]);
    assert_eq!(state.consume(&capture, b"/deny\n"), vec![]);
    assert_eq!(
        state.consume(&capture, b"\n"),
        vec![RawInputEvent::CardApprove("req-1".to_string())]
    );
}

#[test]
fn question_capture_still_submits_selected_option() {
    let capture = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 2,
        allow_free_text: true,
        multiple: false,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b"\t\n"),
        vec![
            RawInputEvent::CardFocus("q-1".to_string(), 1),
            RawInputEvent::CardAnswer("2".to_string())
        ]
    );
}

#[test]
fn question_capture_multiple_toggles_options_and_submits_indices() {
    let capture = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 3,
        allow_free_text: true,
        multiple: true,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b" \t \n"),
        vec![
            RawInputEvent::CardToggle("q-1".to_string(), 0),
            RawInputEvent::CardFocus("q-1".to_string(), 1),
            RawInputEvent::CardToggle("q-1".to_string(), 1),
            RawInputEvent::CardAnswer("1,2".to_string())
        ]
    );
}

#[test]
fn question_capture_multiple_preserves_checked_options_with_custom_answer() {
    let capture = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 3,
        allow_free_text: true,
        multiple: true,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b" \t\t\tDocs\n"),
        vec![
            RawInputEvent::CardToggle("q-1".to_string(), 0),
            RawInputEvent::CardFocus("q-1".to_string(), 1),
            RawInputEvent::CardFocus("q-1".to_string(), 2),
            RawInputEvent::CardFocus("q-1".to_string(), 3),
            RawInputEvent::CardInput("q-1".to_string(), "D".to_string()),
            RawInputEvent::CardInput("q-1".to_string(), "Do".to_string()),
            RawInputEvent::CardInput("q-1".to_string(), "Doc".to_string()),
            RawInputEvent::CardInput("q-1".to_string(), "Docs".to_string()),
            RawInputEvent::CardAnswer("1\nDocs".to_string())
        ]
    );
}

#[test]
fn mode_capture_moves_focus_and_submits_selected_option() {
    let capture = RawInputCapture::Mode {
        id: "mode".to_string(),
        option_count: 2,
        selected: 0,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b"\x1b[C\n"),
        vec![
            RawInputEvent::ModeFocus("mode".to_string(), 1),
            RawInputEvent::ModeSet("mode".to_string(), 1)
        ]
    );
}

#[test]
fn mode_capture_uses_initial_selected_option() {
    let capture = RawInputCapture::Mode {
        id: "mode".to_string(),
        option_count: 2,
        selected: 1,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b"\n"),
        vec![RawInputEvent::ModeSet("mode".to_string(), 1)]
    );
}

#[test]
fn config_capture_saves_default_selection_and_cancels_second_option() {
    let capture = RawInputCapture::Config {
        id: "config".to_string(),
        option_count: 2,
        selected: 0,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b"\n"),
        vec![RawInputEvent::ConfigSave("config".to_string())]
    );

    state.apply_capture(&capture);
    assert_eq!(
        state.consume(&capture, b"\x1b[C\n"),
        vec![
            RawInputEvent::ConfigFocus("config".to_string(), 1),
            RawInputEvent::ConfigCancel("config".to_string())
        ]
    );
}

#[test]
fn config_language_capture_selects_language_and_cancels() {
    let capture = RawInputCapture::ConfigLanguage {
        id: "config-language".to_string(),
        option_count: 3,
        selected: 0,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b"\x1b[C\x1b[C\n"),
        vec![
            RawInputEvent::ConfigLanguageFocus("config-language".to_string(), 1),
            RawInputEvent::ConfigLanguageFocus("config-language".to_string(), 2),
            RawInputEvent::ConfigLanguageSet("config-language".to_string(), 2)
        ]
    );

    state.apply_capture(&capture);
    assert_eq!(
        state.consume(&capture, b"\x1b\n"),
        vec![RawInputEvent::ConfigLanguageCancel(
            "config-language".to_string()
        )]
    );
}

#[test]
fn approval_capture_handles_split_escape_arrow_sequence() {
    let capture = RawInputCapture::Approval {
        id: "req-1".to_string(),
        is_hook: false,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert!(state.consume(&capture, b"\x1b").is_empty());
    assert!(state.consume(&capture, b"[").is_empty());
    assert_eq!(
        state.consume(&capture, b"C\n"),
        vec![
            RawInputEvent::CardFocus("req-1".to_string(), 1),
            RawInputEvent::CardAlwaysTrust("req-1".to_string())
        ]
    );
}

#[test]
fn approval_capture_escape_then_enter_cancels_without_submit() {
    let capture = RawInputCapture::Approval {
        id: "req-1".to_string(),
        is_hook: false,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert!(state.consume(&capture, b"\x1b").is_empty());
    assert_eq!(
        state.consume(&capture, b"\n"),
        vec![RawInputEvent::CardCancel("req-1".to_string())]
    );
}

#[test]
fn question_capture_ctrl_c_and_escape_cancel_question() {
    let capture = RawInputCapture::Question {
        id: "q-1".to_string(),
        option_count: 2,
        allow_free_text: true,
        multiple: false,
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, &[0x03]),
        vec![RawInputEvent::QuestionCancel("q-1".to_string())]
    );

    state.apply_capture(&capture);
    assert!(state.consume(&capture, b"\x1b").is_empty());
    assert_eq!(
        state.consume(&capture, b"\n"),
        vec![RawInputEvent::QuestionCancel("q-1".to_string())]
    );
}

#[test]
fn evidence_capture_sends_ignores_and_cancels() {
    let capture = RawInputCapture::Evidence {
        id: "evidence-1".to_string(),
    };
    let mut state = CardInputState::default();
    state.apply_capture(&capture);

    assert_eq!(
        state.consume(&capture, b"\n"),
        vec![RawInputEvent::EvidenceSend("evidence-1".to_string())]
    );

    state.apply_capture(&capture);
    assert_eq!(
        state.consume(&capture, b"i"),
        vec![RawInputEvent::EvidenceIgnore("evidence-1".to_string())]
    );

    state.apply_capture(&capture);
    assert_eq!(
        state.consume(&capture, &[0x03]),
        vec![RawInputEvent::EvidenceCancel("evidence-1".to_string())]
    );
}
