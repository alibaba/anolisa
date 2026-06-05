use super::*;

#[test]
fn question_panel_renders_options_with_compact_instructions() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let options = vec![
        "Green".to_string(),
        "Blue".to_string(),
        "Custom".to_string(),
    ];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-1",
            question: "Choose a color for the next step",
            options: &options,
            selected_option: 1,
            selected_options: &[],
            custom_answer: "",
            allow_free_text: false,
            selection_mode: QuestionSelectionMode::Single,
        })
        .join("\n");

    assert!(text.contains("Agent question"), "{text}");
    assert!(!text.contains("Agent question q-1"), "{text}");
    assert!(text.contains("Choose a color for the next step"), "{text}");
    assert!(text.contains("Select one:"), "{text}");
    assert!(text.contains("[1] Green"), "{text}");
    assert!(text.contains("[2] Blue"), "{text}");
    assert!(text.contains("> [2] Blue"), "{text}");
    assert!(text.contains("Keys:"), "{text}");
    assert!(text.contains("Left/Right move"), "{text}");
    assert!(text.contains("Enter send"), "{text}");
    assert!(!text.contains("│Choose"), "{text}");
    assert!(!text.contains("│Options"), "{text}");
    assert!(!text.contains("Agent is asking for input"), "{text}");
    assert!(!text.contains("same Agent session"), "{text}");
    assert!(!text.contains("/answer"), "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn question_panel_renders_multiple_choice_toggles() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let options = vec![
        "Lint".to_string(),
        "Unit tests".to_string(),
        "Raw shell smoke".to_string(),
    ];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-1",
            question: "Choose checks to run",
            options: &options,
            selected_option: 1,
            selected_options: &[0, 1],
            custom_answer: "",
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Multiple,
        })
        .join("\n");

    assert!(text.contains("Choose checks to run"), "{text}");
    assert!(text.contains("Select one or more:"), "{text}");
    assert!(text.contains("[x] [1] Lint"), "{text}");
    assert!(text.contains("[x] [2] Unit tests"), "{text}");
    assert!(text.contains("[ ] [3] Raw shell smoke"), "{text}");
    assert!(text.contains("[4] Other..."), "{text}");
    assert!(text.contains("Space toggle"), "{text}");
    assert!(text.contains("Enter send"), "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn question_panel_keeps_cjk_and_emoji_borders_aligned() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let options = vec![
        "分析 CPU 占用 🧪".to_string(),
        "分析内存占用并保留同一会话".to_string(),
        "只解释刚才失败的命令".to_string(),
    ];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-宽",
            question: "请选择下一步要分析的方向，允许补充自定义说明",
            options: &options,
            selected_option: 3,
            selected_options: &[0, 1],
            custom_answer: "重点看中文路径和表格边线",
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Multiple,
        })
        .join("\n");

    assert!(text.contains("Agent question"), "{text}");
    assert!(text.contains("Select one or more:"), "{text}");
    assert!(text.contains("[x] [1] 分析 CPU 占用 🧪"), "{text}");
    assert!(text.contains("[x] [2] 分析内存占用"), "{text}");
    assert!(text.contains("> [4] Other: 重点看中文路径"), "{text}");
    assert!(text.contains("type answer"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn question_panel_wraps_long_question_and_options_without_dropping_tail() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let options = vec![
        "Use the short safe command and keep the same provider session".to_string(),
        "Explain the failure first, then ask before any tool request".to_string(),
    ];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-2",
            question: "Choose how the Agent should continue after the failing command while preserving shell-first control",
            options: &options,
            selected_option: 1,
            selected_options: &[],
            custom_answer: "",
            allow_free_text: false,
            selection_mode: QuestionSelectionMode::Single,
        })
        .join("\n");

    assert!(text.contains("after the"), "{text}");
    assert!(text.contains("failing command"), "{text}");
    assert!(text.contains("shell-first"), "{text}");
    assert!(text.contains("control"), "{text}");
    assert!(text.contains("same"), "{text}");
    assert!(text.contains("provider session"), "{text}");
    assert!(text.contains("│        provider session"), "{text}");
    assert!(text.contains("│       any tool request"), "{text}");
    assert!(text.contains("before"), "{text}");
    assert!(text.contains("any tool request"), "{text}");
    assert!(text.contains("tool request"), "{text}");
    assert!(text.contains("Left/Right"), "{text}");
    assert!(text.contains("Enter send"), "{text}");
    assert!(!text.contains("session; no shell command runs"), "{text}");
    assert_rendered_width(&text, 54);
}

#[test]
fn question_panel_free_text_only_omits_fake_option_section() {
    let renderer = RatatuiInlineRenderer::with_width(90);
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-4",
            question: "Tell me the branch name to inspect",
            options: &[],
            selected_option: 0,
            selected_options: &[],
            custom_answer: "",
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Single,
        })
        .join("\n");

    assert!(text.contains("Agent question"), "{text}");
    assert!(!text.contains("Agent question q-4"), "{text}");
    assert!(
        text.contains("Tell me the branch name to inspect"),
        "{text}"
    );
    assert!(text.contains("Type answer"), "{text}");
    assert!(text.contains("Enter send"), "{text}");
    assert!(!text.contains("Answer:"), "{text}");
    assert!(!text.contains("Type an answer"), "{text}");
    assert!(!text.contains("[1]"), "{text}");
    assert_rendered_width(&text, 90);
}

#[test]
fn question_panel_write_preserves_ratatui_styles_for_terminal_output() {
    let renderer = RatatuiInlineRenderer {
        width: 100,
        plain: false,
        styled: true,
    };
    let options = vec!["Approve".to_string(), "Deny".to_string()];
    let mut output = Vec::new();

    renderer
        .write_question_panel(
            &mut output,
            QuestionPanelModel {
                id: "q-1",
                question: "Choose an answer",
                options: &options,
                selected_option: 1,
                selected_options: &[],
                custom_answer: "",
                allow_free_text: false,
                selection_mode: QuestionSelectionMode::Single,
            },
        )
        .expect("render question panel");

    let text = String::from_utf8(output).expect("utf8 panel");
    let clean = strip_ansi_escape(&text);
    assert!(text.contains("\x1b["), "{text:?}");
    assert!(clean.contains("Agent question"), "{clean}");
    assert!(!clean.contains("Agent question q-1"), "{clean}");
    assert!(clean.contains("[2]"), "{clean}");
}

#[test]
fn plain_question_panel_keeps_compact_card_instructions() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let options = vec!["Green".to_string(), "Blue".to_string()];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-1",
            question: "Choose a color",
            options: &options,
            selected_option: 0,
            selected_options: &[],
            custom_answer: "",
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Single,
        })
        .join("\n");

    assert!(text.contains("Agent question"), "{text}");
    assert!(!text.contains("Agent question q-1"), "{text}");
    assert!(text.contains("[1] Green"), "{text}");
    assert!(text.contains("[3] Other..."), "{text}");
    assert!(text.contains("> [1] Green"), "{text}");
    assert!(text.contains("Left/Right move"), "{text}");
    assert!(text.contains("Enter send"), "{text}");
    assert!(!text.contains("Agent is asking for input"), "{text}");
    assert!(!text.contains("Effect:"), "{text}");
    assert!(!text.contains('╭'), "{text}");
}

#[test]
fn plain_question_panel_wraps_long_question_and_options() {
    let renderer = RatatuiInlineRenderer::plain_with_width(50);
    let options = vec![
        "Use the short safe command and keep the same provider session".to_string(),
        "Explain the failure first, then ask before any tool request".to_string(),
    ];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-2",
            question: "Choose how the Agent should continue after the failing command while preserving shell-first control",
            options: &options,
            selected_option: 2,
            selected_options: &[],
            custom_answer: "",
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Single,
        })
        .join("\n");

    assert!(text.contains("Agent question"), "{text}");
    assert!(!text.contains("Agent question q-2"), "{text}");
    assert!(
        text.contains("Choose how the Agent should continue after the"),
        "{text}"
    );
    assert!(
        text.contains("failing command while preserving shell-first"),
        "{text}"
    );
    assert!(text.contains("control"), "{text}");
    assert!(
        text.contains("  [1] Use the short safe command and keep the"),
        "{text}"
    );
    assert!(text.contains("      same provider session"), "{text}");
    assert!(
        text.contains("  [2] Explain the failure first, then ask"),
        "{text}"
    );
    assert!(text.contains("      before any tool request"), "{text}");
    assert!(text.contains("> [3] Other..."), "{text}");
    assert!(
        text.contains("Left/Right move | type answer | Enter"),
        "{text}"
    );
    assert!(text.contains("send"), "{text}");
    assert_rendered_width(&text, 50);
}

#[test]
fn question_panel_renders_custom_answer_as_selectable_option() {
    let renderer = RatatuiInlineRenderer::with_width(90);
    let options = vec!["Green".to_string(), "Blue".to_string()];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-3",
            question: "Choose a color or provide your own",
            options: &options,
            selected_option: 2,
            selected_options: &[],
            custom_answer: "",
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Single,
        })
        .join("\n");

    assert!(text.contains("[1] Green"), "{text}");
    assert!(text.contains("[2] Blue"), "{text}");
    assert!(text.contains("[3] Other..."), "{text}");
    assert!(text.contains("> [3] Other..."), "{text}");
    assert!(text.contains("type answer"), "{text}");
    assert!(text.contains("Enter send"), "{text}");
    assert_rendered_width(&text, 90);
}

#[test]
fn question_panel_renders_custom_answer_input_value() {
    let renderer = RatatuiInlineRenderer::with_width(90);
    let options = vec!["Green".to_string(), "Blue".to_string()];
    let text = renderer
        .question_panel_lines(QuestionPanelModel {
            id: "q-3",
            question: "Choose a color or provide your own",
            options: &options,
            selected_option: 2,
            selected_options: &[],
            custom_answer: "红色",
            allow_free_text: true,
            selection_mode: QuestionSelectionMode::Single,
        })
        .join("\n");

    assert!(text.contains("> [3] Other: 红色"), "{text}");
    assert!(!text.contains("Custom answer"), "{text}");
    assert_rendered_width(&text, 90);
}

#[test]
fn question_answer_panel_renders_same_session_receipt() {
    let renderer = RatatuiInlineRenderer::with_width(56);
    let text = renderer
        .question_answer_panel_lines(QuestionAnswerPanelModel {
            id: "q-1",
            question: "Choose a color for the next step while keeping shell-first control",
            answer: "蓝色",
            message: "Sent to Agent; no command ran.",
        })
        .join("\n");

    assert!(text.contains("Answer"), "{text}");
    assert!(!text.contains("Answer sent"), "{text}");
    assert!(text.contains("Answer: 蓝色"), "{text}");
    assert!(!text.contains("Sent to Agent"), "{text}");
    assert!(!text.contains("no command ran"), "{text}");
    assert!(!text.contains("same Agent session"), "{text}");
    assert!(!text.contains("no command was executed"), "{text}");
    assert!(!text.contains("Question:"), "{text}");
    assert!(!text.contains("shell-first control"), "{text}");
    assert!(!text.contains("│Answer"), "{text}");
    assert!(!text.contains('┌'), "{text}");
    assert!(!text.contains('└'), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(!text.contains("q-1 Choose a color"), "{text}");
    assert_rendered_width(&text, 56);
}

#[test]
fn plain_question_answer_panel_wraps_long_receipt() {
    let renderer = RatatuiInlineRenderer::plain_with_width(50);
    let text = renderer
        .question_answer_panel_lines(QuestionAnswerPanelModel {
            id: "q-3",
            question: "Choose how the Agent should continue while preserving shell-first control",
            answer: "Run only the smallest safe check and keep the same provider session",
            message: "Sent to Agent; no command ran.",
        })
        .join("\n");

    assert!(text.contains("Answer"), "{text}");
    assert!(!text.contains("Answer sent"), "{text}");
    assert!(
        text.contains("Answer: Run only the smallest safe check and"),
        "{text}"
    );
    assert!(
        text.contains("        keep the same provider session"),
        "{text}"
    );
    assert!(!text.contains("Sent to Agent"), "{text}");
    assert!(!text.contains("no command ran"), "{text}");
    assert!(
        !text.contains("Continuing the same Agent session"),
        "{text}"
    );
    assert!(!text.contains("no command was executed"), "{text}");
    assert!(!text.contains("Question:"), "{text}");
    assert!(!text.contains("preserving shell-first control"), "{text}");
    assert!(!text.contains('╭'), "{text}");
    assert_rendered_width(&text, 50);
}
