//! Contract tests for the `/hooks` status panel, which shares the
//! reference-panel visual contract with `/help`.

use super::{
    strip_ansi_escape, AgentHooksView, HookEntryView, HookEventGroup, HookStatusPanelModel,
    RatatuiInlineRenderer,
};

fn sample_model() -> HookStatusPanelModel<'static> {
    HookStatusPanelModel {
        title: "Hook status",
        shell_label: "Shell Hooks",
        shell_lines: vec![
            "Registered: 2; enabled: 2; disabled: 0.".to_string(),
            "Sources: builtin=2; user=0; project=0.".to_string(),
        ],
        agent_label: "Agent Hooks",
        agent: AgentHooksView::Groups(vec![
            HookEventGroup {
                event: "PreToolUse".to_string(),
                hooks: vec![
                    HookEntryView {
                        name: "skill-ledger".to_string(),
                        extension: "agent-sec-core".to_string(),
                        disabled: false,
                    },
                    HookEntryView {
                        name: "pii-checker".to_string(),
                        extension: "agent-sec-core".to_string(),
                        disabled: true,
                    },
                ],
            },
            HookEventGroup {
                event: "Stop".to_string(),
                hooks: vec![HookEntryView {
                    name: "observability-hook".to_string(),
                    extension: "agent-sec-core".to_string(),
                    disabled: false,
                }],
            },
        ]),
        footer: "3 hook(s) registered.".to_string(),
    }
}

#[test]
fn hook_status_panel_layers_sections_events_and_entries() {
    let renderer = RatatuiInlineRenderer::with_width(80);
    let mut output = Vec::new();
    renderer
        .write_hook_status_panel(&mut output, sample_model())
        .unwrap();

    let text = strip_ansi_escape(&String::from_utf8(output).unwrap());
    let lines = text.lines().collect::<Vec<_>>();
    // Section headers at column zero, like /help group headers.
    assert!(
        lines.iter().any(|line| line.starts_with("│ Shell Hooks")),
        "{text}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("│ Agent Hooks")),
        "{text}"
    );
    // Shell stats indented under their section.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│   Registered: 2")),
        "{text}"
    );
    // Event group headers indented by two, entries by four.
    assert!(
        lines.iter().any(|line| line.starts_with("│   PreToolUse")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│     • skill-ledger (ext: agent-sec-core)")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│     ○ pii-checker (ext: agent-sec-core) [disabled]")),
        "{text}"
    );
    // No legacy per-line event suffix.
    assert!(!text.contains("[PreToolUse]"), "{text}");
    // Footer inside the panel body.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│ 3 hook(s) registered.")),
        "{text}"
    );
}

#[test]
fn hook_status_panel_strips_control_sequences_from_registry_text() {
    // Registry-provided names/extensions and error messages are untrusted:
    // embedded escape sequences must never reach the terminal (they could
    // recolor or clear the screen), on either backend.
    let mut model = sample_model();
    model.agent = AgentHooksView::Groups(vec![HookEventGroup {
        event: "Pre\u{1b}[31mToolUse".to_string(),
        hooks: vec![HookEntryView {
            name: "evil\u{1b}[31mred".to_string(),
            extension: "wipe\u{1b}[2Jext".to_string(),
            disabled: false,
        }],
    }]);

    let mut styled = RatatuiInlineRenderer::with_width(100);
    styled.styled = true;
    let mut output = Vec::new();
    styled.write_hook_status_panel(&mut output, model).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains("\u{1b}[31m"), "{text:?}");
    assert!(!text.contains("\u{1b}[2J"), "{text:?}");
    assert!(text.contains("evilred"), "{text}");
    assert!(
        text.contains("wipeJext") || text.contains("wipeext"),
        "{text}"
    );

    let mut model = sample_model();
    model.agent = AgentHooksView::Message("boom\u{1b}[2J\rcleared".to_string());
    let plain = RatatuiInlineRenderer::plain_with_width(100);
    let mut output = Vec::new();
    plain.write_hook_status_panel(&mut output, model).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains('\u{1b}'), "{text:?}");
    assert!(!text.contains('\r'), "{text:?}");
}

#[test]
fn hook_status_panel_wraps_long_entries_with_hanging_indent_when_narrow() {
    let long_entry = HookEntryView {
        name: "very-long-hook-name-that-wraps".to_string(),
        extension: "very-long-extension-name".to_string(),
        disabled: false,
    };
    let model = HookStatusPanelModel {
        title: "Hook status",
        shell_label: "Shell Hooks",
        shell_lines: vec![
            "Registered: 2; enabled: 2; disabled: 0. Sources: builtin=2; user=0; project=0."
                .to_string(),
        ],
        agent_label: "Agent Hooks",
        agent: AgentHooksView::Groups(vec![HookEventGroup {
            event: "PreToolUse".to_string(),
            hooks: vec![long_entry],
        }]),
        footer: "1 hook(s) registered.".to_string(),
    };

    let renderer = RatatuiInlineRenderer::with_width(40);
    let mut output = Vec::new();
    renderer
        .write_hook_status_panel(&mut output, model)
        .unwrap();

    let text = strip_ansi_escape(&String::from_utf8(output).unwrap());
    let lines = text.lines().collect::<Vec<_>>();
    // The entry wraps; its continuation keeps the hanging indent instead of
    // falling back to column zero.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│     • very-long-hook-name")),
        "{text}"
    );
    let entry_index = lines
        .iter()
        .position(|line| line.starts_with("│     • very-long-hook-name"))
        .unwrap();
    assert!(
        lines[entry_index + 1].starts_with("│       "),
        "continuation lost its hanging indent: {text}"
    );
    // Shell stats continuations stay indented under their section too.
    let stats_index = lines
        .iter()
        .position(|line| line.starts_with("│   Registered: 2"))
        .unwrap();
    assert!(
        lines[stats_index + 1].starts_with("│   "),
        "stats continuation returned to column zero: {text}"
    );

    // Plain backend wraps to the same width contract instead of overflowing.
    let plain = RatatuiInlineRenderer::plain_with_width(40);
    let model = HookStatusPanelModel {
        title: "Hook status",
        shell_label: "Shell Hooks",
        shell_lines: vec!["Registered: 2; enabled: 2; disabled: 0.".to_string()],
        agent_label: "Agent Hooks",
        agent: AgentHooksView::Groups(vec![HookEventGroup {
            event: "PreToolUse".to_string(),
            hooks: vec![HookEntryView {
                name: "very-long-hook-name-that-wraps".to_string(),
                extension: "very-long-extension-name".to_string(),
                disabled: false,
            }],
        }]),
        footer: "1 hook(s) registered.".to_string(),
    };
    let mut output = Vec::new();
    plain.write_hook_status_panel(&mut output, model).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.lines().all(|line| line.chars().count() <= 60),
        "plain line overflowed the width contract: {text}"
    );
    assert!(text.contains("very-long-hook-name-that-wraps"), "{text}");
}

#[test]
fn hook_status_panel_message_view_and_plain_backend_degrade() {
    let mut model = sample_model();
    model.agent = AgentHooksView::Message("(none)".to_string());

    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let mut output = Vec::new();
    renderer
        .write_hook_status_panel(&mut output, model)
        .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Hook status:"), "{text}");
    assert!(text.contains("── Shell Hooks ──"), "{text}");
    assert!(text.contains("── Agent Hooks ──"), "{text}");
    assert!(text.contains("(none)"), "{text}");
    assert!(text.contains("3 hook(s) registered."), "{text}");
    assert!(!text.contains('│'), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
}
