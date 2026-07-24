//! Contract tests for the `/help` panel and the notice-panel indentation
//! guarantees that its layered layout depends on.

use super::{
    strip_ansi_escape, HelpPanelEntry, HelpPanelGroup, HelpPanelModel, NoticePanelModel,
    RatatuiInlineRenderer,
};

#[test]
fn rich_notice_panel_preserves_leading_indent_and_hanging_wrap() {
    let renderer = RatatuiInlineRenderer::with_width(44);
    let mut output = Vec::new();

    renderer
        .write_notice_panel(
            &mut output,
            NoticePanelModel {
                title: "Slash commands",
                body: vec![
                    "Config".to_string(),
                    "  /config language - configure the UI language of the shell [config]"
                        .to_string(),
                ],
                footer: None,
            },
        )
        .unwrap();

    let text = String::from_utf8(output).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    // Group header stays at column zero inside the panel.
    assert!(
        lines.iter().any(|line| line.starts_with("│ Config")),
        "{text}"
    );
    // The entry keeps its two-space indent instead of being trimmed flat.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│   /config language")),
        "{text}"
    );
    // Wrapped continuation lines keep the same hanging indent as the entry,
    // so they never sit at column zero where group headers live.
    let indented = lines.iter().filter(|line| line.starts_with("│   ")).count();
    assert!(indented >= 2, "{text}");
}

#[test]
fn help_panel_layers_groups_entries_scopes_and_summaries() {
    let renderer = RatatuiInlineRenderer::with_width(80);
    let mut output = Vec::new();

    renderer
        .write_help_panel(
            &mut output,
            HelpPanelModel {
                title: "Slash commands",
                groups: vec![
                    HelpPanelGroup {
                        label: "Config",
                        entries: vec![HelpPanelEntry {
                            usage: "/config language [auto|en-US|zh-CN]",
                            summary: "configure UI language",
                            scope: "config",
                        }],
                    },
                    HelpPanelGroup {
                        label: "Modes",
                        entries: vec![HelpPanelEntry {
                            usage: "/mode analysis [smart|auto|manual]",
                            summary: "choose suggested mode, automatic analysis, or no proactive assistance; controls passive suggestions and failure insights after failed commands",
                            scope: "session",
                        }],
                    },
                ],
                footer: "Mode: auto. Strategy: smart.".to_string(),
            },
        )
        .unwrap();

    let text = strip_ansi_escape(&String::from_utf8(output).unwrap());
    let lines = text.lines().collect::<Vec<_>>();
    // Group headers at column zero; entries indented by two spaces.
    assert!(
        lines.iter().any(|line| line.starts_with("│ Config")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│   /config language [auto|en-US|zh-CN]")),
        "{text}"
    );
    // Scope tags right-aligned against the border.
    assert!(
        lines
            .iter()
            .any(|line| line.trim_end().ends_with("[config] │")),
        "{text}"
    );
    // Summaries on their own line with a six-space indent, wrapped with a
    // hanging indent that never reaches column zero.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│       configure UI language")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│       choose suggested mode")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .filter(|line| line.starts_with("│       "))
            .count()
            >= 3,
        "{text}"
    );
    // A blank spacer row separates groups.
    let config_index = lines
        .iter()
        .position(|line| line.starts_with("│ Config"))
        .unwrap();
    let modes_index = lines
        .iter()
        .position(|line| line.starts_with("│ Modes"))
        .unwrap();
    assert!(
        lines[config_index + 1..modes_index]
            .iter()
            .any(|line| line.chars().all(|ch| ch == '│' || ch == ' ') && line.contains('│')),
        "{text}"
    );
    // Footer is rendered inside the panel body.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│ Mode: auto. Strategy: smart.")),
        "{text}"
    );
}

#[test]
fn plain_help_panel_keeps_group_structure_without_styles() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let mut output = Vec::new();

    renderer
        .write_help_panel(
            &mut output,
            HelpPanelModel {
                title: "Slash commands",
                groups: vec![HelpPanelGroup {
                    label: "Config",
                    entries: vec![HelpPanelEntry {
                        usage: "/config language [auto|en-US|zh-CN]",
                        summary: "configure UI language",
                        scope: "config",
                    }],
                }],
                footer: "Mode: auto. Strategy: smart.".to_string(),
            },
        )
        .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Slash commands:"), "{text}");
    assert!(text.contains("── Config ──"), "{text}");
    assert!(
        text.contains("/config language [auto|en-US|zh-CN] [config]"),
        "{text}"
    );
    assert!(text.contains("configure UI language"), "{text}");
    assert!(!text.contains('│'), "{text}");
}

#[test]
fn styled_agent_response_wraps_long_paragraph_without_truncation() {
    // P1 regression: with the styled channel not re-wrapping, unwrapped
    // markdown paragraphs were truncated at the panel edge on narrow styled
    // terminals. The tail sentinel must survive.
    let mut renderer = RatatuiInlineRenderer::with_width(40);
    renderer.styled = true;
    let mut output = Vec::new();

    let text = "This is a deliberately long plain paragraph that must wrap across \
                several lines inside a narrow styled panel and finally end with \
                TAIL-SENTINEL";
    renderer
        .write_agent_response(&mut output, text, None)
        .unwrap();

    let rendered = strip_ansi_escape(&String::from_utf8(output).unwrap());
    assert!(rendered.contains("TAIL-SENTINEL"), "{rendered}");
}

#[test]
fn styled_block_emits_no_ansi_when_styles_disabled() {
    // P2 regression: styled panels must respect styles_enabled(); NO_COLOR or
    // non-TTY output must not contain escape sequences.
    let renderer = RatatuiInlineRenderer::with_width(80);
    assert!(!renderer.styles_enabled());
    let mut output = Vec::new();

    renderer
        .write_help_panel(
            &mut output,
            HelpPanelModel {
                title: "Slash commands",
                groups: vec![HelpPanelGroup {
                    label: "Config",
                    entries: vec![HelpPanelEntry {
                        usage: "/config language [auto|en-US|zh-CN]",
                        summary: "configure UI language",
                        scope: "config",
                    }],
                }],
                footer: "Mode: auto. Strategy: smart.".to_string(),
            },
        )
        .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(text.contains("│ Config"), "{text}");
}
